//! Persistent block-backed filesystem manager.
//!
//! Owns the physical placement policy for the InterFS region so the filesystem
//! cannot overlap the boot area or the persistent metadata journal.
use crate::storage::block::BlockDevice;
use crate::drivers::ATA_PRIMARY;
use super::boot_guard::{self, SectorRange, WriteVerdict};
use super::interfs::{FsBackendError, InterFs};

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

fn filesystem_region_sectors(total: u32) -> Result<u32, FsBackendError> {
    region_for_layout(total, FilesystemLayout::canonical())
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
fn guard_region(
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
            Ok(())
        }
        Err(FsBackendError::InvalidFormat) => { let _ = InterFs::format_region(device, layout.base_lba, region)?; Ok(()) }
        Err(FsBackendError::UnsupportedVersion) => {
            // The region holds a filesystem this build cannot read. It is not
            // mounted and not interpreted -- reading another version's fields
            // as this version's would return another format's files as if
            // they were this one's -- and the region is formatted afresh.
            crate::serial_println!(
                "[INTERFS] the filesystem on this device was written in another format version; the region is being reformatted."
            );
            let _ = InterFs::format_region(device, layout.base_lba, region)?;
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
}
