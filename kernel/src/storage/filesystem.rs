// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Persistent block-backed filesystem manager.
//!
//! Owns the physical placement policy for the InterFS region so the filesystem
//! cannot overlap the boot area or the persistent metadata journal.
use crate::storage::block::BlockDevice;
use crate::drivers::ATA_PRIMARY;
use super::boot_guard::{self, SectorRange, WriteVerdict};
use super::interfs::{FsBackendError, FsLayout, InterFs, SpaceReport};

/// Leave the first 4096 sectors to the bootloader/partition/device-specific area.
pub const BLOCK_FS_BASE_LBA: u32 = 4096;
/// Sectors the persistent journal occupies at the disk tail.
///
/// WHY this is derived from `persistent::TOTAL_SLOT_COUNT` rather than written
/// as its own literal: the journal grew an extension pair when kind 9 was added,
/// and a hand-written `16 * 128` silently under-reserved, which would have let
/// InterFS format over the verified-boot record if the InterFS size cap were ever
/// raised. Deriving both from the journal's own slot count makes that drift
/// impossible.
pub const BLOCK_FS_TAIL_RESERVED: u32 =
    super::persistent::SLOT_SECTORS * super::persistent::TOTAL_SLOT_COUNT;

/// Smallest region the filesystem can be formatted on.
///
/// WHY it is a minimum and not a maximum: the region used to be capped so the
/// eight-sector bitmap could address it, which pinned every filesystem to
/// 16.8 MiB of data however large the disk was. The bitmap now grows with the
/// region, so the only bound left is the one the geometry itself imposes.
pub const MIN_INTERFS_REGION_SECTORS: u32 = super::interfs::MIN_REGION_SECTORS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemLayout {
    pub base_lba: u32,
    pub tail_reserved_sectors: u32,
}

impl FilesystemLayout {
    pub const fn canonical() -> Self { Self { base_lba: BLOCK_FS_BASE_LBA, tail_reserved_sectors: BLOCK_FS_TAIL_RESERVED } }
}

/// The sectors of the device the filesystem region may use.
///
/// WHY there is no upper bound: the region's geometry is derived from its own
/// size, so a 6 GiB disk yields a 6 GiB region and a 512 MiB disk a 512 MiB
/// one. The only limits are the boot area in front and the journal tail
/// behind.
fn region_for_layout(total: u32, layout: FilesystemLayout) -> Result<u32, FsBackendError> {
    let end = total.checked_sub(layout.tail_reserved_sectors).ok_or(FsBackendError::NoSpace)?;
    let available = end.checked_sub(layout.base_lba)
        .filter(|v| *v >= MIN_INTERFS_REGION_SECTORS)
        .ok_or(FsBackendError::NoSpace)?;
    Ok(available)
}

/// The sectors the canonical InterFS region occupies on a device of `total`.
///
/// WHY `pub(super)` and not private: the provisioning path in `staging` streams
/// hundreds of thousands of sectors and must mount the filesystem ONCE, not once
/// per chunk. Remounting per chunk re-reads the whole allocation bitmap -- 3,071
/// sectors on the demo disk -- and under emulated IDE that is not slow, it is
/// unreachable. A single mount has to be made through this module's region and
/// guard, or the streaming path would be a second, unguarded way into the region.
pub(super) fn filesystem_region_sectors(total: u32) -> Result<u32, FsBackendError> {
    region_for_layout(total, FilesystemLayout::canonical())
}

/// The largest file the filesystem on a `total_sectors` device can ever hold.
///
/// WHY this is the data region and not the free space: InterFS gives a file one
/// contiguous extent, so no inode can describe more bytes than the region holds
/// whatever the disk looks like. That is the format's ceiling. The ceiling
/// *today* is the largest contiguous free run, which `space_report` measures and
/// which is always at most this. Reporting only the format's ceiling would let
/// a used disk claim room it no longer has.
///
/// WHY it takes a sector count rather than a device: the bound is pure geometry,
/// so a caller that has not opened the disk yet -- a build tool deciding whether
/// a model is worth copying, a boot report naming the limit -- can state it.
///
/// # Concrete sizes on the demo data disk
///
/// The emulator builds a 6 GiB data disk (`emulator/server.py`, "the guest's 6GB
/// data disk"), 12,582,912 sectors. This function returns 6,437,453,824 bytes
/// for it. `embeddinggemma-300M-Q8_0.gguf` is 333,590,944 bytes, which is 5.18%
/// of that, so the published model is inside the bound with 19x to spare and no
/// on-disk format change is needed to hold it.
pub fn max_data_file_bytes(total_sectors: u32) -> Option<u64> {
    let region = region_for_layout(total_sectors, FilesystemLayout::canonical()).ok()?;
    FsLayout::for_region(region).map(|layout| layout.capacity_bytes())
}

/// The largest file the filesystem on `device` can ever hold, in bytes.
pub fn max_data_file_bytes_with_device(device: &dyn BlockDevice) -> u64 {
    max_data_file_bytes(device.sector_count()).unwrap_or(0)
}

/// The largest file the data device this kernel is running on can hold.
///
/// WHY this is the one a surface should ask: a screen that recites a model-size
/// limit is making a claim about hardware it did not read, and the number is
/// wrong the moment the disk is a different size. This asks the mounted device.
///
/// WHY it returns `None` rather than `0` when there is no device: "this device
/// holds no room" and "this device was never attached" are different statements,
/// and a report that says the first when the second is true is a fabricated
/// capacity claim. A caller with `None` says the device is absent.
///
/// WHY no bitmap walk is involved: this is the format's ceiling, which the
/// geometry gives from the sector count. The ceiling *right now* -- the largest
/// contiguous free run -- is `reserve_data_file_checked_with_device`, and the two
/// differ on any disk that has been written to.
pub fn max_data_file_bytes_on_device() -> Option<u64> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut()?;
    Some(max_data_file_bytes_with_device(device))
}

/// Says, on the console the operator reads, what the disk was measured to hold.
///
/// WHY the numbers are printed and not only returned: a refusal to store a model
/// is reached from a streaming writer that has already spent its time copying,
/// and the one thing that decides its next move is whether it asked for more than
/// the format can express or merely more than this disk currently has in one run.
/// Both are bytes; a bare `NoSpace` makes the reader guess. `boot_guard` already
/// announces its refusals this way, and a device-verified capacity claim belongs
/// on the same channel as the device-verified refusal.
fn announce_space_refusal(scope: &str, path: &str, report: &SpaceReport) {
    crate::serial_println!(
        "[INTERFS] {} refuses {}: asked {} bytes, largest contiguous extent available is {} bytes, {} bytes free in total, data region {} bytes.",
        scope, path, report.asked_bytes, report.largest_free_run_bytes,
        report.free_bytes, report.data_region_bytes
    );
    if report.asked_bytes > report.data_region_bytes {
        crate::serial_println!(
            "[INTERFS] the ask exceeds what one InterFS extent can describe on this device; no amount of free space would take it."
        );
    } else {
        crate::serial_println!(
            "[INTERFS] the disk holds the bytes but not in one run; the model is refused rather than promised a multi-extent file this format does not have."
        );
    }
}

/// Decides whether the InterFS region of `device` may be written at all.
///
/// WHY this runs before the mount attempt and not only around the format: the
/// region is written on every subsequent `store_data_file`, and `mount_region`
/// itself publishes a superblock when it recovers an interrupted transaction.
/// Guarding only `format_region` would have left a filesystem formatted by an
/// earlier boot as a live way to overwrite the kernel image.
///
/// WHY the caller supplies the device and the range and the guard derives both
/// facts itself: the range comes from the layout policy above, the image extent
/// from the running kernel's program headers, and the boot area from the
/// device's own partition table. Nothing here consults a fixed sector number,
/// so repacking the image moves the protection with it.
pub(super) fn guard_region(
    device: &mut dyn BlockDevice,
    base_lba: u32,
    region_sectors: u32,
    scope: &str,
) -> Result<(), FsBackendError> {
    let Some(target) = SectorRange::new(base_lba, region_sectors) else {
        return Err(FsBackendError::NoSpace);
    };
    match boot_guard::evaluate_device_writes(device, target) {
        WriteVerdict::Permitted => Ok(()),
        WriteVerdict::Refused(refusal) => {
            let mut sector = [0u8; crate::storage::block::SECTOR_SIZE];
            // WHY re-read LBA 0 for the notice: the refusal path needs the same
            // boot area the decision used, and a failed read is itself a
            // refusal reason, so a failure here yields no boot area rather than
            // a fabricated one.
            let boot = if device.read_sector(0, &mut sector).is_ok() {
                Some(boot_guard::parse_boot_area(&sector))
            } else {
                None
            };
            boot_guard::announce_refusal(scope, device, target, boot.as_ref(), refusal);
            Err(FsBackendError::BootDeviceProtected)
        }
    }
}

/// Mount the canonical InterFS region, formatting it only when no valid
/// filesystem exists. Formatting is deliberately bounded away from the boot
/// area and journal tail, and is refused outright when the bound is not enough
/// to clear the device the kernel booted from.
pub fn init_block_filesystem_with_device(device: &mut dyn BlockDevice) -> Result<(), FsBackendError> {
    init_block_filesystem_with_layout(device, FilesystemLayout::canonical())
}

/// States, on the console the boot log is taken from, how large a file the
/// filesystem the platform just read can hold.
///
/// WHY this is reported at boot: the question a model payload raises is not
/// "is there a loader" but "how large a file can this device be given", and an
/// answer that is only available by reading the allocator source is an answer
/// nobody acting on the device can check. The number comes from the superblock
/// the mount already read, so it states what was observed on this disk rather
/// than what the geometry would predict for a disk of this size -- which on a
/// device that has been resized are different things, and the difference is the
/// part worth printing.
///
/// WHY no free-space figure appears here: the longest contiguous free run needs
/// a full pass over the allocation bitmap, 3,071 sector reads on the demo disk.
/// Boot is not the place to spend that on every start, and the number changes
/// the moment anything is written. It is measured by
/// `reserve_data_file_checked_with_device`, on the ask that acts on it.
fn announce_observed_capacity<D: BlockDevice + ?Sized>(scope: &str, fs: &InterFs<'_, D>) {
    crate::serial_println!(
        "[INTERFS] {} region: {} data sectors, {} bytes -- the largest file one InterFS extent can hold on this device.",
        scope, fs.capacity_sectors(), fs.capacity_bytes()
    );
}

pub fn init_block_filesystem_with_layout(device: &mut dyn BlockDevice, layout: FilesystemLayout) -> Result<(), FsBackendError> {
    let region = region_for_layout(device.sector_count(), layout)?;
    guard_region(device, layout.base_lba, region, "InterFS format")?;
    match InterFs::mount_region(device, layout.base_lba, region) {
        Ok(fs) => {
            // A device that grew since the filesystem was formatted keeps the
            // filesystem it has; the sectors past it are simply not part of
            // this filesystem. A device that shrank would cut it in half, and
            // the mount refuses that before it gets here.
            if fs.region_sectors() > region {
                return Err(FsBackendError::CorruptMetadata);
            }
            announce_observed_capacity("mounted", &fs);
            Ok(())
        }
        Err(FsBackendError::InvalidFormat) => {
            let fs = InterFs::format_region(device, layout.base_lba, region)?;
            announce_observed_capacity("formatted", &fs);
            Ok(())
        }
        Err(FsBackendError::UnsupportedVersion) => {
            // The region holds a filesystem this build cannot read. It is not
            // mounted and not interpreted -- reading another version's fields
            // as this version's would return another format's files as if
            // they were this one's -- and the region is formatted afresh.
            crate::serial_println!(
                "[INTERFS] the filesystem on this device was written in another format version; the region is being reformatted."
            );
            let fs = InterFs::format_region(device, layout.base_lba, region)?;
            announce_observed_capacity("reformatted", &fs);
            Ok(())
        }
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
    // WHY the guard applies to a read as well: `mount_region` runs crash
    // recovery, which republishes the superblock. There is no read-only mount in
    // this backend, so "open it" and "write to it" are the same operation.
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS mount")?;
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
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS write")?;
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

/// Reserves room for `capacity` bytes in a data file, creating it if needed.
///
/// WHY a reservation rather than a plain write: a file that is larger than the
/// kernel heap is written a chunk at a time, and each chunk has to land
/// inside an extent that is already allocated. Reserving the extent once
/// means the chunks are in-place sector writes.
pub fn reserve_data_file(path: &str, capacity: u64) -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    reserve_data_file_with_device(device, path, capacity)
}

/// Reserves room for `capacity` bytes in a data file on `device`.
pub fn reserve_data_file_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
    capacity: u64,
) -> Result<(), FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS write")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.reserve_stream(path, capacity)
}

/// Reserves `capacity` bytes for `path` only if the disk is measured to hold
/// that much in the one contiguous extent InterFS would give it.
///
/// WHY this exists beside `reserve_data_file_with_device` rather than replacing
/// it: the streaming writer's first call is the reservation, and a payload
/// larger than the kernel heap is copied in after that call succeeds. Refusing
/// halfway through -- when the extent turns out not to be there -- would leave a
/// partial payload on the device and a caller holding nothing to act on. This
/// measures the disk first, and on refusal names the bytes asked and the bytes
/// observed, then leaves the disk exactly as it found it.
///
/// WHAT THE BOUND IS: the reported `SpaceReport` is measured from the on-disk
/// allocation bitmap, so the bound this enforces is the real one -- one extent,
/// at most the data region, and no more than the longest free run the disk holds
/// at the moment of the ask. It is not a constant and it is not the free total.
///
/// WHY the returned report is the pre-reservation one: it is the number the
/// caller needs to print or refuse on. After the reservation the run it measured
/// is consumed, so a post-call measurement would report a different, smaller,
/// disk and read as though the write had failed.
pub fn reserve_data_file_checked(
    path: &str,
    capacity: u64,
) -> Result<SpaceReport, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    reserve_data_file_checked_with_device(device, path, capacity)
}

/// Reserves `capacity` bytes for `path` on `device`, refusing with measured
/// numbers when the disk cannot hold the one extent.
pub fn reserve_data_file_checked_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
    capacity: u64,
) -> Result<SpaceReport, FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS write")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    let report = fs.space_report(capacity)?;
    if !report.fits() {
        announce_space_refusal("reserve", path, &report);
        return Err(if capacity > report.data_region_bytes {
            FsBackendError::FileTooLarge
        } else {
            FsBackendError::NoSpace
        });
    }
    fs.reserve_stream(path, capacity)?;
    Ok(report)
}

/// Writes `data` at `offset` inside a data file, growing the file if needed.
pub fn store_data_file_range(path: &str, offset: u64, data: &[u8]) -> Result<usize, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    store_data_file_range_with_device(device, path, offset, data)
}

/// Writes `data` at `offset` inside a data file on `device`.
pub fn store_data_file_range_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
    offset: u64,
    data: &[u8],
) -> Result<usize, FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS write")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.write_at(path, offset, data)
}

/// Reads up to `out.len()` bytes from a data file at `offset`.
pub fn load_data_file_range(path: &str, offset: u64, out: &mut [u8]) -> Result<usize, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    load_data_file_range_with_device(device, path, offset, out)
}

/// Reads up to `out.len()` bytes from a data file on `device` at `offset`.
pub fn load_data_file_range_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
    offset: u64,
    out: &mut [u8],
) -> Result<usize, FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS mount")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.read_at(path, offset, out)
}

/// Resizes a data file to `new_size` bytes, keeping the bytes it still holds.
///
/// WHY this is not "write the first `new_size` bytes back": a file that is
/// larger than the kernel heap cannot be read into memory to be resized, which
/// is the same reason the payload is streamed in the first place. The block
/// backend resizes the published length in place.
pub fn truncate_data_file(path: &str, new_size: u64) -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    truncate_data_file_with_device(device, path, new_size)
}

/// Resizes a data file on `device` to `new_size` bytes.
pub fn truncate_data_file_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
    new_size: u64,
) -> Result<(), FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS write")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.truncate_file(path, new_size)
}

/// The published size of a data file, in bytes.
pub fn data_file_len(path: &str) -> Result<u64, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    data_file_len_with_device(device, path)
}

/// The published size of a data file on `device`, in bytes.
///
/// WHY this is the streaming path's commit point: a chunk write publishes the
/// file's length, so a reader that asks the length knows how much of the
/// payload actually landed. The VFS takes a device-held file's length from here
/// rather than from the snapshot record, because the record was written before
/// the reset and the device was not.
pub fn data_file_len_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
) -> Result<u64, FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS mount")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.file_len(path)
}

/// Removes a data file and the blocks it held.
pub fn remove_data_file(path: &str) -> Result<(), FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    remove_data_file_with_device(device, path)
}

/// Removes a data file on `device` and the blocks it held.
///
/// WHY the `_with_device` twin exists: the rest of this module's helpers all have
/// one, and the bound this layer enforces is only testable if the fragmentation
/// that exercises it can be built without the real disk. Removing a file is the
/// half that hands the sectors back.
pub fn remove_data_file_with_device(
    device: &mut dyn BlockDevice,
    path: &str,
) -> Result<(), FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS write")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    match fs.delete_file(path) {
        Ok(()) => Ok(()),
        Err(FsBackendError::FileNotFound) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The device-side file a provisioning tool leaves the list of model payloads at.
///
/// WHY a device-side name and not a VFS path: a file the kernel can only see
/// once something tells it the file exists cannot be discovered by looking for
/// it, and the VFS's own record of what exists is sealed in the keystore, so a
/// host tool cannot write one. This path lives in the block filesystem alone,
/// which is the one place a companion tool and the kernel both can reach, and
/// no application opens files on the block filesystem directly.
///
/// WHY it is not under `/data`: every `/data/` name the kernel owns is either a
/// VFS payload spill or a snapshot, and a fourth kind of file sharing those
/// prefixes would be read as one of them by the restore. `/provision/` is a
/// namespace the VFS never creates, so a file here is only ever the manifest.
pub const PROVISION_MANIFEST_PATH: &str = "/provision/models.list";

/// The most manifest bytes the kernel reads at boot.
///
/// WHY a window and not the file's length: the point of the read is that a
/// provisioning record costs a bounded amount of work no matter what else the
/// device holds. A line the window cut short fails the entry's own validation
/// and is reported, so a manifest this side of the limit is imported in full
/// and one past it is imported as far as it was read, with the difference said
/// out loud.
pub const PROVISION_MANIFEST_WINDOW: usize = 1024;

// WHY a compile-time gate on a string that names a place on the block device:
// the record packs the path into a fixed field and refuses a longer one, so a
// manifest renamed past that bound would not be missing -- it would be
// unaskable, and every boot would report no provisioned model forever.
const _: () = assert!(
    PROVISION_MANIFEST_PATH.len() <= super::interfs::MAX_PATH,
    "the provisioning manifest path must fit one InterFS inode record"
);

/// Reads the head of the provisioning manifest, if the device holds one.
///
/// WHY the missing-manifest case is not an error worth a console line: the
/// ordinary device has no provisioned model, and a boot-time line saying so on
/// every boot would train anyone reading the log to ignore it. The caller acts
/// on the refusal and says nothing.
pub fn load_provision_manifest(out: &mut [u8]) -> Result<usize, FsBackendError> {
    let mut guard = ATA_PRIMARY.lock();
    let device = guard.as_mut().ok_or(FsBackendError::NoSpace)?;
    load_provision_manifest_with_device(device, out)
}

/// Reads the head of the provisioning manifest from `device`.
pub fn load_provision_manifest_with_device(
    device: &mut dyn BlockDevice,
    out: &mut [u8],
) -> Result<usize, FsBackendError> {
    let region = filesystem_region_sectors(device.sector_count())?;
    guard_region(device, BLOCK_FS_BASE_LBA, region, "InterFS mount")?;
    let mut fs = InterFs::mount_region(device, BLOCK_FS_BASE_LBA, region)?;
    fs.read_at(PROVISION_MANIFEST_PATH, 0, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::block::{BlockDeviceError, SECTOR_SIZE};
    use alloc::vec;
    use alloc::vec::Vec;

    /// Sector count of the packaged reference image: a 4 MiB FAT boot
    /// partition that starts at LBA 962 and runs to the end of the file.
    const PACKAGED_SECTORS: u32 = 9154;

    /// In-memory device that records every write, so a test can prove the disk
    /// was left untouched rather than only that an error came back.
    struct MockDevice {
        sectors: Vec<u8>,
        total: u32,
        writes: usize,
    }

    impl MockDevice {
        fn blank(total: u32) -> Self {
            Self { sectors: vec![0u8; total as usize * SECTOR_SIZE], total, writes: 0 }
        }

        /// A device that presents itself as boot media, with the given
        /// partition table.
        fn boot_media_with(total: u32, parts: &[(u32, u32)]) -> Self {
            let mut dev = Self::blank(total);
            let mut mbr = [0u8; SECTOR_SIZE];
            for (i, (lba, count)) in parts.iter().enumerate() {
                let o = 446 + i * 16;
                mbr[o] = 0x80;
                mbr[o + 4] = 0x0C;
                mbr[o + 8..o + 12].copy_from_slice(&lba.to_le_bytes());
                mbr[o + 12..o + 16].copy_from_slice(&count.to_le_bytes());
            }
            mbr[510] = 0x55;
            mbr[511] = 0xAA;
            dev.sectors[..SECTOR_SIZE].copy_from_slice(&mbr);
            dev
        }

        /// The packaged reference image: a loader partition at LBA 1 and a
        /// 4 MiB FAT partition at LBA 962 running to the end of the file. The
        /// kernel ELF lives inside that FAT partition.
        fn packaged_boot_image() -> Self {
            Self::boot_media_with(PACKAGED_SECTORS, &[(1, 961), (962, 8192)])
        }

        fn sector(&self, lba: u32) -> &[u8] {
            let start = lba as usize * SECTOR_SIZE;
            &self.sectors[start..start + SECTOR_SIZE]
        }
    }

    impl BlockDevice for MockDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            buf.copy_from_slice(self.sector(lba));
            Ok(())
        }
        fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            let start = lba as usize * SECTOR_SIZE;
            self.sectors[start..start + SECTOR_SIZE].copy_from_slice(buf);
            self.writes += 1;
            Ok(())
        }
        fn sector_count(&self) -> u32 { self.total }
        fn model(&self) -> &str { "MOCK" }
    }

    /// The defect this pins: on the packaged single-disk image the canonical
    /// InterFS region starts at LBA 4096, which is inside the FAT boot partition
    /// that holds `kernel-x86_64`. Before the guard existed this call formatted
    /// 41 metadata sectors there plus a data extent, replacing 22,437 bytes of
    /// the running image and faulting the *next* boot.
    #[test]
    fn formatting_the_boot_device_is_refused_and_writes_nothing() {
        let mut dev = MockDevice::packaged_boot_image();
        let before = dev.sectors.clone();
        assert_eq!(dev.writes, 0);

        let result = init_block_filesystem_with_device(&mut dev);

        assert_eq!(result, Err(FsBackendError::BootDeviceProtected));
        assert_eq!(dev.writes, 0, "a refused format must not issue a single write");
        assert_eq!(dev.sectors, before, "the device must be byte-identical afterwards");
        // The evidence the refusal is protecting: LBA 0 still holds the boot
        // sector, and the filesystem base still holds whatever it held.
        assert_eq!(dev.sector(0)[510], 0x55);
    }

    #[test]
    fn the_same_layout_is_still_formatted_on_a_device_with_no_boot_sector() {
        // A data disk has no 0x55AA boot sector, so no boot chain can have
        // started from it and it is safe to initialise. Refusing here would have
        // turned a protection into a loss of persistence.
        let mut dev = MockDevice::blank(PACKAGED_SECTORS);
        assert_eq!(init_block_filesystem_with_device(&mut dev), Ok(()));
        assert!(dev.writes > 0);
        assert_eq!(&dev.sector(BLOCK_FS_BASE_LBA)[0..4], b"IOFS");
    }

    /// WHY this is the test that shows the guard is not "never format LBA 4096":
    /// the same device, the same partition table, the same canonical base -- and
    /// a filesystem placed past the advertised boot area formats normally. Only
    /// the overlap decides.
    #[test]
    fn a_layout_past_the_boot_area_formats_even_on_boot_media() {
        // Boot area ends at LBA 9154 per this partition table, so place the
        // filesystem beyond it.
        let mut dev = MockDevice::boot_media_with(65536, &[(1, 961), (962, 8192)]);
        let layout = FilesystemLayout { base_lba: 16384, tail_reserved_sectors: BLOCK_FS_TAIL_RESERVED };
        let before = dev.sectors.clone();
        assert_eq!(init_block_filesystem_with_layout(&mut dev, layout), Ok(()));
        assert!(dev.writes > 0);
        assert_eq!(&dev.sector(16384)[0..4], b"IOFS");
        // ...and nothing below the boot area was touched.
        assert_eq!(&dev.sectors[..16384 * SECTOR_SIZE], &before[..16384 * SECTOR_SIZE]);
    }

    /// The counterpart: move that same layout back inside the advertised boot
    /// area and the identical call is refused.
    #[test]
    fn the_same_layout_inside_the_boot_area_is_refused() {
        let mut dev = MockDevice::boot_media_with(65536, &[(1, 961), (962, 8192)]);
        let layout = FilesystemLayout { base_lba: 8192, tail_reserved_sectors: BLOCK_FS_TAIL_RESERVED };
        let before = dev.sectors.clone();
        assert_eq!(
            init_block_filesystem_with_layout(&mut dev, layout),
            Err(FsBackendError::BootDeviceProtected)
        );
        assert_eq!(dev.writes, 0);
        assert_eq!(dev.sectors, before);
    }

    #[test]
    fn a_write_through_store_is_refused_on_the_boot_device_too() {
        // Formatting is not the only way to destroy the image: every later
        // `store_data_file` writes the same region.
        let mut dev = MockDevice::packaged_boot_image();
        let before = dev.sectors.clone();
        assert_eq!(
            store_data_file_with_device(&mut dev, "/data/test", b"payload"),
            Err(FsBackendError::BootDeviceProtected)
        );
        assert_eq!(dev.writes, 0);
        assert_eq!(dev.sectors, before);
    }

    /// The backstop inside the destructive primitive itself. A caller that skips
    /// the placement layer -- the serial console, a recovery path, a future tool
    /// -- must not be able to format the boot device either.
    #[test]
    fn format_region_itself_refuses_the_boot_device() {
        let mut dev = MockDevice::packaged_boot_image();
        let before = dev.sectors.clone();
        let result = InterFs::format_region(&mut dev, BLOCK_FS_BASE_LBA, 2754);
        assert!(matches!(result, Err(FsBackendError::BootDeviceProtected)), "{:?}", result.err());
        assert_eq!(dev.writes, 0);
        assert_eq!(dev.sectors, before);
    }

    /// ...and it still formats a data device, so the backstop does not simply
    /// disable the operation.
    #[test]
    fn format_region_still_formats_a_data_device() {
        let mut dev = MockDevice::blank(PACKAGED_SECTORS);
        let fs = InterFs::format_region(&mut dev, BLOCK_FS_BASE_LBA, 2754).expect("data device must format");
        // The geometry now follows the region: a superblock, one bitmap
        // sector, the inode table and everything else is data.
        let expected = super::super::interfs::FsLayout::for_region(2754).expect("a usable region");
        assert_eq!(fs.capacity_sectors(), expected.data_sectors);
        assert_eq!(fs.capacity_sectors(), 2754 - 34);
        assert_eq!(&dev.sector(BLOCK_FS_BASE_LBA)[0..4], b"IOFS");
    }

    /// The defect this pins: the region used to be capped at the size the
    /// eight-sector bitmap could address, so a large disk was formatted with
    /// 16.8 MiB of usable data and the rest of it was unreachable.
    #[test]
    fn the_region_follows_the_size_of_the_device() {
        // A 6 GiB device: 12,582,912 sectors.
        //
        // WHY the multiplication is written the way it is: 6 * 1024 * 1024 * 1024
        // overflows a `u32` before the division by the sector size, and an
        // arithmetic overflow in a constant is an error rather than a wrong
        // number.
        let total = 6u32 * 1024 * 1024 * 2;
        let region = region_for_layout(total, FilesystemLayout::canonical())
            .expect("a 6 GiB device must have a filesystem region");
        assert!(
            region > 12_000_000,
            "a 6 GiB device must yield a region of its own size, not a capped one: {region}",
        );
        assert!(region > MIN_INTERFS_REGION_SECTORS);
    }

    /// The streamed path the VFS uses for a payload it must not hold: reserve
    /// once, write a chunk at a time, read a range back, resize in place. None
    /// of these calls ever carries the whole file, and none of them is the
    /// whole-file `store_data_file`, which would need the payload in memory.
    #[test]
    fn a_streamed_payload_round_trips_through_the_range_helpers() {
        let path = "/data/models/streamed.gguf";
        let mut dev = MockDevice::blank(PACKAGED_SECTORS);
        init_block_filesystem_with_device(&mut dev).expect("format a data device");

        reserve_data_file_with_device(&mut dev, path, 512 * 1024).expect("reserve the extent");
        // A reservation is not a payload: the file exists and holds nothing.
        assert_eq!(data_file_len_with_device(&mut dev, path).expect("length"), 0);

        let chunk: Vec<u8> = (0..4096usize).map(|i| (i % 251) as u8).collect();
        assert_eq!(
            store_data_file_range_with_device(&mut dev, path, 0, &chunk).expect("first chunk"),
            chunk.len()
        );
        assert_eq!(
            store_data_file_range_with_device(&mut dev, path, 4096, &chunk).expect("second chunk"),
            chunk.len()
        );
        assert_eq!(data_file_len_with_device(&mut dev, path).expect("length"), 8192);

        let mut out = alloc::vec![0u8; 4096];
        assert_eq!(load_data_file_range_with_device(&mut dev, path, 4096, &mut out).expect("read the second chunk"), 4096);
        assert_eq!(out[..], chunk[..], "the chunk reads back byte for byte at its offset");

        // Shrink and grow: the resize is what the next mount sees, and the
        // grown region is zeroes rather than whatever the extent held before.
        truncate_data_file_with_device(&mut dev, path, 2048).expect("shrink");
        assert_eq!(data_file_len_with_device(&mut dev, path).expect("length"), 2048);
        truncate_data_file_with_device(&mut dev, path, 300 * 1024).expect("grow");
        assert_eq!(data_file_len_with_device(&mut dev, path).expect("length"), 300 * 1024);

        let mut head = [0u8; 64];
        load_data_file_range_with_device(&mut dev, path, 0, &mut head).expect("read the head");
        assert_eq!(head[..], chunk[..64], "the bytes before the shrink survived");
        let mut grown = [0u8; 512];
        load_data_file_range_with_device(&mut dev, path, 100 * 1024, &mut grown).expect("read the grown region");
        assert!(grown.iter().all(|b| *b == 0), "the region past the end is zero-filled");

        // A range write to a file the store does not have is refused rather
        // than silently creating one with no extent behind it.
        assert_eq!(
            store_data_file_range_with_device(&mut dev, "/data/models/absent.gguf", 0, &chunk).err(),
            Some(FsBackendError::FileNotFound),
        );
    }

    /// The boot guard is not bypassed by the streaming helpers: a payload can
    /// only be streamed into a region the placement layer already allows.
    #[test]
    fn the_streaming_helpers_refuse_the_boot_device() {
        let mut dev = MockDevice::packaged_boot_image();
        let before = dev.sectors.clone();
        assert_eq!(
            reserve_data_file_with_device(&mut dev, "/data/models/x.gguf", 4096),
            Err(FsBackendError::BootDeviceProtected)
        );
        assert_eq!(dev.writes, 0);
        assert_eq!(dev.sectors, before);
    }

    /// A device that stores only the sectors that were written.
    ///
    /// WHY this is not the `MockDevice` above: proving that the published model
    /// file fits one extent means exercising the 6 GiB geometry, and a device
    /// holding every sector of it in memory would need six gigabytes of host
    /// RAM to prove one number. Sparse, the same proof costs the 333 MB actually
    /// written.
    struct SparseDevice {
        total: u32,
        sectors: std::collections::BTreeMap<u32, [u8; SECTOR_SIZE]>,
        writes: u64,
    }

    impl SparseDevice {
        fn new(total: u32) -> Self {
            Self { total, sectors: std::collections::BTreeMap::new(), writes: 0 }
        }
    }

    impl BlockDevice for SparseDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            match self.sectors.get(&lba) {
                Some(sector) => buf.copy_from_slice(sector),
                None => buf.fill(0),
            }
            Ok(())
        }
        fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            self.sectors.insert(lba, *buf);
            self.writes += 1;
            Ok(())
        }
        fn sector_count(&self) -> u32 { self.total }
        fn model(&self) -> &str { "sparse" }
    }

    /// 6 GiB in sectors, as the emulator builds the data disk.
    const SIX_GB_SECTORS: u32 = 6 * 1024 * 1024 * 2;

    /// The published model: `target/verify/models/embeddinggemma-300M-Q8_0.gguf`,
    /// measured from the file on this host rather than quoted from a ticket.
    const MODEL_BYTES: u64 = 333_590_944;

    /// The byte at an offset of the streamed payload.
    ///
    /// WHY a function of position and not a table: the payload must never be
    /// resident. A 333 MB `Vec` would make the test prove the filesystem holds
    /// the file by spending the same memory the test is meant to fit in, which
    /// is the property the kernel cannot have either.
    fn payload_byte(offset: u64) -> u8 {
        let x = offset.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 29;
        (x as u8) ^ ((offset >> 7) as u8) ^ ((offset >> 13) as u8)
    }

    /// The defect this pins: the claim that a 333 MB model cannot be created
    /// because InterFS allocates contiguous extents only.
    ///
    /// WHAT THE CODE ACTUALLY DOES: `DiskInode::sector_count` is a `u32` of
    /// sectors, so the field reaches 2 TiB, and the only bound `create_stream`
    /// applies is `sectors > data_sectors`. On the 6 GiB demo disk that region is
    /// 12,573,152 sectors, and `format_region` zeroes the bitmap, so the whole
    /// region is one free run and the model needs 651,545 of it -- 5.18%.
    ///
    /// So multi-extent allocation is not what this file needs, and adding it would
    /// have moved a persisted on-disk format for no reason. The bound is the disk.
    ///
    /// WHY a whole round trip and not a reservation: a reservation only proves the
    /// bitmap was marked. This writes every one of the 333,590,944 bytes, in
    /// 2,545 chunks that never hold more than 128 KiB at once, then reads all of
    /// them back and compares byte for byte. The assertion names the counts so a
    /// regression that silently truncates cannot pass by accident.
    #[test]
    fn a_333_megabyte_model_round_trips_through_one_extent() {
        let mut dev = SparseDevice::new(SIX_GB_SECTORS);
        init_block_filesystem_with_device(&mut dev).expect("format the demo data disk");

        // The bound the design rests on, measured off the disk that was just
        // formatted -- not recited from a constant.
        let max = max_data_file_bytes_with_device(&mut dev);
        assert_eq!(
            max, 6_437_453_824,
            "the 6 GiB demo disk's one-extent ceiling is the data region, in bytes"
        );
        assert!(
            MODEL_BYTES < max,
            "a {MODEL_BYTES} byte model needs {MODEL_BYTES} bytes of run and the region holds {max}"
        );

        let path = "/data/models/embeddinggemma-300M-Q8_0.gguf";
        assert!(
            path.len() <= super::super::interfs::MAX_PATH,
            "the real model name has to fit one inode record, or it is unaskable: {} vs {}",
            path.len(), super::super::interfs::MAX_PATH,
        );

        let report = reserve_data_file_checked_with_device(&mut dev, path, MODEL_BYTES)
            .expect("the published model must be reservable in the extent the format gives");
        assert_eq!(report.asked_bytes, MODEL_BYTES);
        assert_eq!(
            report.largest_free_run_bytes, 12_573_152 * 512,
            "a fresh region is one free run, so the largest extent available is all of it"
        );
        assert!(report.fits());

        // Stream the payload in. 128 KiB is the most this holds at one time.
        const CHUNK: usize = 128 * 1024;
        let mut chunk = vec![0u8; CHUNK];
        let mut written: u64 = 0;
        let mut chunks = 0u64;
        while written < MODEL_BYTES {
            let want = core::cmp::min(CHUNK as u64, MODEL_BYTES - written) as usize;
            for i in 0..want {
                chunk[i] = payload_byte(written + i as u64);
            }
            let n = store_data_file_range_with_device(&mut dev, path, written, &chunk[..want])
                .unwrap_or_else(|e| panic!("chunk at {written} landed: {e:?}"));
            assert_eq!(n, want, "the store takes every byte it was handed");
            written += want as u64;
            chunks += 1;
        }
        assert_eq!(written, MODEL_BYTES, "every byte of the model was handed to the store");
        assert_eq!(
            data_file_len_with_device(&mut dev, path).expect("the published length"),
            MODEL_BYTES,
            "the file publishes its full {} bytes", MODEL_BYTES,
        );

        // Read all of it back and compare, without ever holding the file.
        let mut read: u64 = 0;
        let mut reads = 0u64;
        while read < MODEL_BYTES {
            let want = core::cmp::min(CHUNK as u64, MODEL_BYTES - read) as usize;
            let n = load_data_file_range_with_device(&mut dev, path, read, &mut chunk[..want])
                .expect("the range reads back");
            assert_eq!(n, want, "the read returns the bytes it asked for at {read}");
            for i in 0..want {
                let at = read + i as u64;
                assert_eq!(chunk[i], payload_byte(at), "byte {} of the model differs", at);
            }
            read += want as u64;
            reads += 1;
        }
        assert_eq!(
            read, MODEL_BYTES,
            "{} bytes streamed in {} chunks of at most {} bytes, and all {} bytes read back byte for byte over {} reads",
            MODEL_BYTES, chunks, CHUNK, read, reads,
        );

        // The extent the model holds is the whole ask in one run, and nothing
        // else on the disk moved.
        let after = reserve_data_file_checked_with_device(&mut dev, path, 512)
            .expect("an existing file keeps its extent");
        assert!(
            after.largest_free_run_bytes < report.largest_free_run_bytes,
            "the model's extent came out of the free run: {} -> {}",
            report.largest_free_run_bytes, after.largest_free_run_bytes,
        );
    }

    /// The bound is the largest contiguous run, not the free total -- and the
    /// refusal says so in bytes.
    ///
    /// WHY this test exists beside the round trip: the round trip proves a fresh
    /// disk takes the model. It does not prove the enforcement is real. A device
    /// that has written and deleted files holds the same total free space in more
    /// than one piece, and a file needing more than any single piece must be
    /// refused, because InterFS gives a file one extent and this build has no
    /// multi-extent allocation to promise. Reporting the free total as if it were
    /// available would be exactly the capacity claim the platform truth rule
    /// forbids.
    #[test]
    fn a_reservation_is_refused_by_the_largest_run_not_the_free_total() {
        // 32 MiB: region 58,880 sectors, bitmap 15, data start 48, data 58,832.
        let mut dev = SparseDevice::new(65_536);
        init_block_filesystem_with_device(&mut dev).expect("format");

        let kept = 10_000 * 512;
        reserve_data_file_checked_with_device(&mut dev, "/data/models/a.gguf", kept).expect("first");
        reserve_data_file_checked_with_device(&mut dev, "/data/models/b.gguf", kept).expect("second");
        // Removing the first leaves a 10,000-sector hole before a 38,832-sector
        // tail: two runs, one free total.
        remove_data_file_with_device(&mut dev, "/data/models/a.gguf").expect("remove the first");

        // 45,000 sectors: more than either run, less than the free total, and
        // less than the data region, so this is a fragmentation refusal and not
        // a "too large for the format" one.
        let asked = 45_000u64 * 512;
        let err = reserve_data_file_checked_with_device(&mut dev, "/data/models/big.gguf", asked)
            .expect_err("no single run is this wide, so the ask is refused");
        assert_eq!(err, FsBackendError::NoSpace, "the disk has the bytes, just not in one extent");

        // The numbers a caller is refused with are measured, not asserted.
        let mounted = {
            let region = filesystem_region_sectors(dev.sector_count()).expect("region");
            InterFs::mount_region(&mut dev, BLOCK_FS_BASE_LBA, region).expect("mount")
        };
        let mut mounted = mounted;
        let report = mounted.space_report(asked).expect("measure the ask");
        assert_eq!(report.asked_bytes, asked, "45,000 sectors asked");
        assert_eq!(report.largest_free_run_bytes, 38_832 * 512, "the tail run is the widest");
        assert_eq!(report.free_bytes, 48_832 * 512, "both runs together");
        assert_eq!(report.data_region_bytes, 58_832 * 512, "the region the format reaches");
        assert!(!report.fits(), "the widest run is {} bytes and the ask is {}",
            report.largest_free_run_bytes, asked);
        assert!(report.free_bytes > asked, "the refusal is about contiguity, not volume");
        assert!(asked <= report.data_region_bytes, "and the format could describe it, so this is not FileTooLarge");

        // Nothing was written by the refusal: the file does not exist.
        assert_eq!(
            data_file_len_with_device(&mut dev, "/data/models/big.gguf")
                .err(),
            Some(FsBackendError::FileNotFound),
            "a refused reservation leaves no half-file behind"
        );
    }

    /// An ask the format itself cannot describe is refused as too large, and the
    /// refusal says which bound it hit.
    #[test]
    fn an_extent_larger_than_the_data_region_is_refused_as_too_large() {
        let mut dev = SparseDevice::new(65_536);
        init_block_filesystem_with_device(&mut dev).expect("format");
        let region_bytes = max_data_file_bytes_with_device(&mut dev);
        assert_eq!(region_bytes, 58_832 * 512, "the 32 MiB device's data region");
        let asked = region_bytes + 512;
        assert_eq!(
            reserve_data_file_checked_with_device(&mut dev, "/data/models/too.gguf", asked).err(),
            Some(FsBackendError::FileTooLarge),
            "{asked} bytes past a {region_bytes} byte region is beyond one extent, not beyond free space",
        );
    }

    /// The documented bound, stated as the number it is.
    ///
    /// WHY the exact figure and not just "large enough": the question this
    /// answers is "can the device hold the published model", and an answer that
    /// cannot be checked against a byte count is not an answer. 6,437,453,824 is
    /// 12,573,152 sectors, which is the 6 GiB disk less the 4,096-sector boot area,
    /// the 2,560-sector journal tail, the superblock, the 3,071-sector bitmap and
    /// the 32-sector inode table.
    #[test]
    fn the_demo_disk_extent_bound_is_the_number_the_geometry_gives() {
        assert_eq!(
            max_data_file_bytes(SIX_GB_SECTORS),
            Some(6_437_453_824),
            "a 6 GiB data disk bounds one file at the data region, in bytes",
        );
        assert!(
            MODEL_BYTES <= 6_437_453_824,
            "the {} byte model is inside the {} byte bound; {} sectors of {}",
            MODEL_BYTES, 6_437_453_824u64,
            (MODEL_BYTES + 511) / 512, 12_573_152,
        );
        // A disk with no room for a region has no bound to claim, and says so
        // rather than reporting a capacity it does not have.
        assert_eq!(max_data_file_bytes(1024), None, "a 1024 sector device holds no region");
    }

    /// The on-disk format did not move for this, and an existing disk still
    /// mounts.
    ///
    /// WHY this is asserted rather than assumed: Option A was chosen because the
    /// measured geometry already reaches the model, so no inode field changed and
    /// `FS_VERSION` stayed at 2. A disk the device already booted from therefore
    /// reads back unchanged. If a later change does move the record, the version
    /// has to move with it and the test has to say so out loud.
    #[test]
    fn the_inode_record_is_unchanged_so_existing_disks_still_mount() {
        assert_eq!(super::super::interfs::FS_VERSION, 2, "the version this build writes");
        assert_eq!(super::super::interfs::MAX_PATH, 46, "the path field of the v2 record");
        assert_eq!(super::super::interfs::INODE_SECTORS, 32, "the v2 inode table");
        // Every byte of the model's own size, written and read on a fresh disk,
        // then re-read by a second mount of the same disk.
        let mut dev = SparseDevice::new(2_097_152); // 1 GiB
        init_block_filesystem_with_device(&mut dev).expect("format");
        let path = "/data/models/remount.gguf";
        reserve_data_file_checked_with_device(&mut dev, path, 1024 * 1024).expect("reserve 1 MiB");
        let chunk: Vec<u8> = (0..1024 * 1024u64).map(payload_byte).collect();
        store_data_file_range_with_device(&mut dev, path, 0, &chunk).expect("write 1 MiB");
        // A second mount of the same disk -- what the next boot does.
        let region = filesystem_region_sectors(dev.sector_count()).expect("region");
        let mut fs = InterFs::mount_region(&mut dev, BLOCK_FS_BASE_LBA, region)
            .expect("a disk written by this build remounts");
        assert_eq!(fs.file_len(path).expect("the file is there"), 1024 * 1024);
        let mut head = [0u8; 512];
        fs.read_at(path, 0, &mut head).expect("the head reads");
        for (i, b) in head.iter().enumerate() {
            assert_eq!(*b, payload_byte(i as u64), "byte {} survived the remount", i);
        }
    }
}
