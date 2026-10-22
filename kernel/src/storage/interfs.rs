//! Minimal on-disk InterOS filesystem backend.
//!
//! Block-backed filesystem boundary above `BlockDevice`.
//! The format is intentionally small and deterministic: a superblock, allocation
//! bitmap, fixed-size inode table, and contiguous file data extents. VFS can use
//! this backend without knowing the physical ATA/SDHCI implementation,
//! with transactional metadata publication and deterministic crash
//! recovery around mutations.
//!
//! # Version 2: a geometry that follows the disk
//!
//! Version 1 hard-coded the geometry. The bitmap was eight sectors, so it
//! could address 32,768 sectors however large the disk was, and the data
//! region started at a fixed LBA right after it. Every filesystem was
//! therefore 16.8 MiB of usable data, on a 512 MiB disk and on a 6 GiB one
//! alike, and a 167 MiB model could not be stored on any of them.
//!
//! Version 2 derives the geometry from the region it is formatted on:
//!
//! ```text
//! LBA 0                       superblock (carries the transaction record)
//! LBA 1 .. 1 + B              allocation bitmap, B = ceil(region / 4096)
//! LBA 1 + B .. 1 + B + 32     inode table (256 inodes of 64 bytes)
//! LBA data_start .. end       file data extents
//! ```
//!
//! One bitmap bit addresses one sector, so one bitmap sector covers 4096
//! sectors and the bitmap grows with the region instead of being a constant.
//! The superblock records every one of those addresses, so the layout is read
//! back from the disk rather than assumed, and `FS_VERSION` separates this
//! format from version 1: a version 1 superblock is *rejected*, never
//! reinterpreted as a version 2 one. Reinterpreting it would be reading a
//! version 1 data region as version 2 extents, and the bytes that come back
//! would be somebody else's file.
//!
//! # Streaming
//!
//! A file larger than the kernel heap is written a chunk at a time, so the
//! backend has to be able to place bytes inside a file that is already on the
//! disk. `create_stream` reserves the extent once, and `write_at` then writes
//! sectors in place; the file only relocates when the reserved extent is too
//! small, and it relocates geometrically so the total copy cost stays linear
//! in the file size.

use crate::crypto::Sha256;
use crate::storage::block::{BlockDevice, BlockDeviceError, SECTOR_SIZE};
use crate::storage::boot_guard::{self, SectorRange, WriteVerdict};

pub const FS_MAGIC: [u8; 4] = *b"IOFS";
pub const FS_VERSION: u32 = 2;
pub const FS_SUPERBLOCK_LBA: u32 = 0;

/// Sectors the superblock occupies. It also carries the transaction record.
const SUPERBLOCK_SECTORS: u32 = 1;
/// Size of one on-disk inode record.
const INODE_SIZE: usize = 64;
/// Inodes packed into one sector.
const INODES_PER_SECTOR: usize = SECTOR_SIZE / INODE_SIZE;
/// Sectors of the inode table: 256 inodes.
pub const INODE_SECTORS: u32 = 32;
/// Inodes the filesystem can hold.
pub const MAX_INODES: u32 = (INODE_SECTORS as usize * INODES_PER_SECTOR) as u32;
/// Path bytes in one inode.
///
/// WHY 46 and not 47: the version 2 inode carries the file size as a 64-bit
/// count, which costs two bytes of the 64-byte record, and the path field
/// takes the rest.
const MAX_PATH: usize = 46;
/// Smallest region that can hold a superblock, a bitmap, the inode table and
/// one data sector.
pub const MIN_REGION_SECTORS: u32 = SUPERBLOCK_SECTORS + 1 + INODE_SECTORS + 1;
const TXN_OFFSET: usize = 64;
const TXN_SIZE: usize = 64;
const TXN_MAGIC: [u8; 4] = *b"IFTX";
const TXN_VERSION: u8 = 2;

/// Transaction operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxnOp {
    /// A new file, with no previous extent.
    Create = 1,
    /// The file's extent is replaced: the old and the new extents differ.
    Write = 2,
    /// The file is removed.
    Delete = 3,
    /// The file's extent is unchanged and only its published size grows.
    ///
    /// WHY a separate operation: the recovery rule for a replaced extent is
    /// "free whichever extent the inode does not point at". For an in-place
    /// extension the old and the new extent are the *same* extent, so that
    /// rule would free the blocks the file still uses. The recovery for this
    /// operation therefore keeps whatever size the inode carries.
    Extend = 4,
}

#[derive(Debug, Clone, Copy)]
struct FsTransaction {
    op: u8,
    inode_index: u32,
    old_used: bool,
    old_size: u64,
    old_start: u32,
    old_count: u32,
    new_used: bool,
    new_size: u64,
    new_start: u32,
    new_count: u32,
}

impl FsTransaction {
    fn encode(&self, out: &mut [u8; TXN_SIZE]) {
        out.fill(0);
        out[0..4].copy_from_slice(&TXN_MAGIC);
        out[4] = TXN_VERSION;
        out[5] = self.op;
        out[8..12].copy_from_slice(&self.inode_index.to_le_bytes());
        out[12] = self.old_used as u8;
        out[13..21].copy_from_slice(&self.old_size.to_le_bytes());
        out[21..25].copy_from_slice(&self.old_start.to_le_bytes());
        out[25..29].copy_from_slice(&self.old_count.to_le_bytes());
        out[29] = self.new_used as u8;
        out[30..38].copy_from_slice(&self.new_size.to_le_bytes());
        out[38..42].copy_from_slice(&self.new_start.to_le_bytes());
        out[42..46].copy_from_slice(&self.new_count.to_le_bytes());
        // The digest covers the record's own fields, so a torn or edited
        // transaction is discarded rather than acted upon.
        let digest = Sha256::digest(&out[..56]);
        out[56..64].copy_from_slice(&digest[..8]);
    }

    fn decode(buf: &[u8; TXN_SIZE]) -> Option<Self> {
        if buf[0..4] != TXN_MAGIC || buf[4] != TXN_VERSION { return None; }
        if Sha256::digest(&buf[..56])[..8] != buf[56..64] { return None; }
        if buf[12] > 1 || buf[29] > 1 { return None; }
        Some(Self {
            op: buf[5],
            inode_index: u32::from_le_bytes([buf[8],buf[9],buf[10],buf[11]]),
            old_used: buf[12] != 0,
            old_size: u64::from_le_bytes([buf[13],buf[14],buf[15],buf[16],buf[17],buf[18],buf[19],buf[20]]),
            old_start: u32::from_le_bytes([buf[21],buf[22],buf[23],buf[24]]),
            old_count: u32::from_le_bytes([buf[25],buf[26],buf[27],buf[28]]),
            new_used: buf[29] != 0,
            new_size: u64::from_le_bytes([buf[30],buf[31],buf[32],buf[33],buf[34],buf[35],buf[36],buf[37]]),
            new_start: u32::from_le_bytes([buf[38],buf[39],buf[40],buf[41]]),
            new_count: u32::from_le_bytes([buf[42],buf[43],buf[44],buf[45]]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsCheckReport { pub used_inodes: u32, pub allocated_sectors: u32, pub referenced_sectors: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsBackendError {
    Io(BlockDeviceError),
    InvalidFormat,
    NoSpace,
    TooManyFiles,
    FileNotFound,
    InvalidPath,
    FileTooLarge,
    CorruptMetadata,
    /// The superblock names a filesystem version this build cannot read.
    ///
    /// WHY a distinct variant rather than `InvalidFormat`: `InvalidFormat`
    /// means "there is no filesystem here", and the mount path answers that by
    /// formatting the region. A device that carries a *foreign* filesystem is
    /// a different situation, and the caller has to be able to tell the two
    /// apart before it destroys anything.
    UnsupportedVersion,
    /// The requested range is inside the bootable region of the device the
    /// kernel is executing from, so it was not written.
    ///
    /// WHY a distinct variant rather than `NoSpace`: no space is being denied.
    /// The write was declined because it would have destroyed the running
    /// image, and the caller has to say so on the console rather than report a
    /// capacity problem the user can do nothing about.
    BootDeviceProtected,
}

impl From<BlockDeviceError> for FsBackendError {
    fn from(value: BlockDeviceError) -> Self { Self::Io(value) }
}

/// The on-disk geometry of a formatted region.
///
/// WHY this is a value and not a set of constants: every address in the
/// format is a function of the region's size, and the superblock records the
/// result. A filesystem therefore has exactly one geometry, and it is the one
/// the disk was formatted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsLayout {
    /// First sector of the allocation bitmap.
    pub bitmap_lba: u32,
    /// Sectors of the allocation bitmap.
    pub bitmap_sectors: u32,
    /// First sector of the inode table.
    pub inode_lba: u32,
    /// Sectors of the inode table.
    pub inode_sectors: u32,
    /// First sector of the data region, relative to the region's base.
    pub data_start: u32,
    /// Sectors of the data region.
    pub data_sectors: u32,
}

impl FsLayout {
    /// The geometry of a region of `region_sectors` sectors.
    ///
    /// WHY the bitmap is sized from the *region* and not from the data
    /// region: the data region's size depends on where it starts, and the data
    /// region starts after the bitmap, so sizing the bitmap from the data
    /// region would be circular. Sizing it from the region is not -- the
    /// region is known before anything is placed -- and it over-allocates by
    /// the bitmap's own size, which is one part in 4096 of the disk.
    pub fn for_region(region_sectors: u32) -> Option<Self> {
        let bitmap_sectors = bitmap_sectors_for(region_sectors);
        let inode_lba = SUPERBLOCK_SECTORS.checked_add(bitmap_sectors)?;
        let data_start = inode_lba.checked_add(INODE_SECTORS)?;
        let data_sectors = region_sectors.checked_sub(data_start)?;
        if data_sectors == 0 { return None; }
        Some(Self {
            bitmap_lba: SUPERBLOCK_SECTORS,
            bitmap_sectors,
            inode_lba,
            inode_sectors: INODE_SECTORS,
            data_start,
            data_sectors,
        })
    }

    /// Bytes the data region can hold.
    pub fn capacity_bytes(&self) -> u64 {
        (self.data_sectors as u64).saturating_mul(SECTOR_SIZE as u64)
    }
}

/// Sectors of allocation bitmap needed to address `region_sectors` sectors.
///
/// One bit per sector, one bitmap sector per 4096 addressed sectors.
fn bitmap_sectors_for(region_sectors: u32) -> u32 {
    let bits = region_sectors as u64;
    let bytes = (bits + 7) / 8;
    let sectors = (bytes + (SECTOR_SIZE as u64 - 1)) / (SECTOR_SIZE as u64);
    // At least one sector, so the bitmap is never empty.
    if sectors == 0 { 1 } else { sectors as u32 }
}

/// Sectors a payload of `bytes` needs.
fn sectors_for(bytes: u64) -> Option<u32> {
    let sectors = bytes.checked_add(SECTOR_SIZE as u64 - 1)? / SECTOR_SIZE as u64;
    u32::try_from(sectors).ok()
}

#[derive(Debug, Clone, Copy)]
struct Superblock {
    /// Sectors of the region this filesystem was formatted on.
    total_sectors: u32,
    layout: FsLayout,
    inode_count: u32,
}

impl Superblock {
    fn encode(&self, out: &mut [u8; SECTOR_SIZE]) {
        out.fill(0);
        out[0..4].copy_from_slice(&FS_MAGIC);
        out[4..8].copy_from_slice(&FS_VERSION.to_le_bytes());
        out[8..12].copy_from_slice(&self.total_sectors.to_le_bytes());
        out[12..16].copy_from_slice(&self.layout.data_start.to_le_bytes());
        out[16..20].copy_from_slice(&self.layout.data_sectors.to_le_bytes());
        out[20..24].copy_from_slice(&self.inode_count.to_le_bytes());
        out[24..28].copy_from_slice(&self.layout.bitmap_lba.to_le_bytes());
        out[28..32].copy_from_slice(&self.layout.bitmap_sectors.to_le_bytes());
        out[32..36].copy_from_slice(&self.layout.inode_lba.to_le_bytes());
        out[36..40].copy_from_slice(&self.layout.inode_sectors.to_le_bytes());
        out[40..44].copy_from_slice(&(SECTOR_SIZE as u32).to_le_bytes());
    }

    fn decode(buf: &[u8; SECTOR_SIZE]) -> Result<Self, FsBackendError> {
        if buf[0..4] != FS_MAGIC {
            // No filesystem here at all: the caller may format the region.
            return Err(FsBackendError::InvalidFormat);
        }
        let version = u32::from_le_bytes([buf[4],buf[5],buf[6],buf[7]]);
        if version != FS_VERSION {
            // A filesystem this build cannot read. It is refused rather than
            // reinterpreted: the fields below mean something else in the other
            // version, and reading them as this version's would return another
            // format's files as if they were this one's.
            return Err(FsBackendError::UnsupportedVersion);
        }
        let total = u32::from_le_bytes([buf[8],buf[9],buf[10],buf[11]]);
        let data_start = u32::from_le_bytes([buf[12],buf[13],buf[14],buf[15]]);
        let data_sectors = u32::from_le_bytes([buf[16],buf[17],buf[18],buf[19]]);
        let inode_count = u32::from_le_bytes([buf[20],buf[21],buf[22],buf[23]]);
        let bitmap_lba = u32::from_le_bytes([buf[24],buf[25],buf[26],buf[27]]);
        let bitmap_sectors = u32::from_le_bytes([buf[28],buf[29],buf[30],buf[31]]);
        let inode_lba = u32::from_le_bytes([buf[32],buf[33],buf[34],buf[35]]);
        let inode_sectors = u32::from_le_bytes([buf[36],buf[37],buf[38],buf[39]]);
        let sector_size = u32::from_le_bytes([buf[40],buf[41],buf[42],buf[43]]);
        if sector_size != SECTOR_SIZE as u32 || total == 0 {
            return Err(FsBackendError::CorruptMetadata);
        }
        // The geometry has to be the one this version derives from the region
        // size, or the superblock is describing a filesystem that does not
        // exist.
        let expected = FsLayout::for_region(total).ok_or(FsBackendError::CorruptMetadata)?;
        if bitmap_lba != expected.bitmap_lba
            || bitmap_sectors != expected.bitmap_sectors
            || inode_lba != expected.inode_lba
            || inode_sectors != expected.inode_sectors
            || data_start != expected.data_start
            || data_sectors != expected.data_sectors
            || inode_count != MAX_INODES
        {
            return Err(FsBackendError::CorruptMetadata);
        }
        Ok(Self { total_sectors: total, layout: expected, inode_count })
    }
}

#[derive(Debug, Clone, Copy)]
struct DiskInode {
    used: bool,
    file_size: u64,
    start_sector: u32,
    sector_count: u32,
    path_len: u8,
    path: [u8; MAX_PATH],
}

impl DiskInode {
    fn empty() -> Self {
        Self { used:false, file_size:0, start_sector:0, sector_count:0, path_len:0, path:[0; MAX_PATH] }
    }

    fn encode(&self, out: &mut [u8; INODE_SIZE]) {
        out.fill(0);
        out[0] = self.used as u8;
        out[1..9].copy_from_slice(&self.file_size.to_le_bytes());
        out[9..13].copy_from_slice(&self.start_sector.to_le_bytes());
        out[13..17].copy_from_slice(&self.sector_count.to_le_bytes());
        out[17] = self.path_len;
        out[18..18+MAX_PATH].copy_from_slice(&self.path);
    }

    fn decode(buf: &[u8; INODE_SIZE]) -> Result<Self, FsBackendError> {
        if buf[0] > 1 || buf[17] as usize > MAX_PATH {
            return Err(FsBackendError::CorruptMetadata);
        }
        let mut path=[0u8; MAX_PATH];
        path.copy_from_slice(&buf[18..18+MAX_PATH]);
        Ok(Self {
            used: buf[0] != 0,
            file_size: u64::from_le_bytes([buf[1],buf[2],buf[3],buf[4],buf[5],buf[6],buf[7],buf[8]]),
            start_sector: u32::from_le_bytes([buf[9],buf[10],buf[11],buf[12]]),
            sector_count: u32::from_le_bytes([buf[13],buf[14],buf[15],buf[16]]),
            path_len: buf[17],
            path,
        })
    }

    fn path(&self) -> Result<&str, FsBackendError> {
        core::str::from_utf8(&self.path[..self.path_len as usize])
            .map_err(|_| FsBackendError::CorruptMetadata)
    }

    fn set_path(&mut self, path: &str) {
        let bytes = path.as_bytes();
        let len = bytes.len().min(MAX_PATH);
        self.path[..len].copy_from_slice(&bytes[..len]);
        self.path_len = len as u8;
    }

    /// Bytes the file's reserved extent can hold.
    fn capacity(&self) -> u64 {
        (self.sector_count as u64).saturating_mul(SECTOR_SIZE as u64)
    }
}

pub struct InterFs<'a, D: BlockDevice + ?Sized> {
    device: &'a mut D,
    superblock: Superblock,
    base_lba: u32,
}

impl<'a, D: BlockDevice + ?Sized> InterFs<'a, D> {
    pub fn format(device: &'a mut D) -> Result<Self, FsBackendError> {
        Self::format_at(device, 0)
    }

    pub fn format_at(device: &'a mut D, base_lba: u32) -> Result<Self, FsBackendError> {
        let total = device.sector_count();
        let region_sectors = total.checked_sub(base_lba).ok_or(FsBackendError::NoSpace)?;
        Self::format_region(device, base_lba, region_sectors)
    }

    pub fn format_region(device: &'a mut D, base_lba: u32, region_sectors: u32) -> Result<Self, FsBackendError> {
        let layout = FsLayout::for_region(region_sectors).ok_or(FsBackendError::NoSpace)?;
        // WHY the guard lives here as well as in `storage::filesystem`: this is
        // the destructive primitive. A caller reaching it directly -- a recovery
        // path, a future tool, the serial console -- must not be able to format
        // the device the kernel is executing from. The placement layer guards
        // first so it can report the refusal with the layout it intended; this
        // one is the backstop for everyone else.
        if let Some(range) = SectorRange::new(base_lba, region_sectors) {
            if let WriteVerdict::Refused(refusal) = boot_guard::evaluate_device_writes(device, range) {
                boot_guard::announce_refusal("InterFS format", device, range, None, refusal);
                return Err(FsBackendError::BootDeviceProtected);
            }
        }
        let sb = Superblock { total_sectors: region_sectors, layout, inode_count: MAX_INODES };
        let mut sector=[0u8; SECTOR_SIZE];
        sb.encode(&mut sector);
        device.write_sector(base_lba + FS_SUPERBLOCK_LBA, &sector)?;

        sector.fill(0);
        for lba in layout.bitmap_lba..layout.bitmap_lba + layout.bitmap_sectors {
            device.write_sector(base_lba + lba, &sector)?;
        }
        for lba in layout.inode_lba..layout.inode_lba + layout.inode_sectors {
            device.write_sector(base_lba + lba, &sector)?;
        }
        Ok(Self { device, superblock: sb, base_lba })
    }

    pub fn mount(device: &'a mut D) -> Result<Self, FsBackendError> {
        Self::mount_at(device, 0)
    }

    pub fn mount_at(device: &'a mut D, base_lba: u32) -> Result<Self, FsBackendError> {
        let available = device.sector_count().checked_sub(base_lba).ok_or(FsBackendError::CorruptMetadata)?;
        Self::mount_region(device, base_lba, available)
    }

    pub fn mount_region(device: &'a mut D, base_lba: u32, region_sectors: u32) -> Result<Self, FsBackendError> {
        // WHY mounting is guarded: `recover_transaction` republishes the
        // superblock, so a mount is not a read. There is no read-only mount in
        // this backend, which means "open the filesystem on this device" and
        // "write to it" are the same decision and have to be guarded together.
        if let Some(range) = SectorRange::new(base_lba, region_sectors) {
            if let WriteVerdict::Refused(refusal) = boot_guard::evaluate_device_writes(device, range) {
                boot_guard::announce_refusal("InterFS mount", device, range, None, refusal);
                return Err(FsBackendError::BootDeviceProtected);
            }
        }
        let mut sector=[0u8; SECTOR_SIZE];
        device.read_sector(base_lba + FS_SUPERBLOCK_LBA, &mut sector)?;
        let sb=Superblock::decode(&sector)?;
        // The geometry is the one the superblock carries, and it describes the
        // region the filesystem was formatted on. A device that has grown since
        // keeps the filesystem it has -- the spare sectors are simply not part
        // of it -- and a device that has *shrank* would cut the filesystem in
        // half, which is refused.
        if sb.total_sectors > region_sectors {
            return Err(FsBackendError::CorruptMetadata);
        }
        let mut fs = Self { device, superblock: sb, base_lba };
        fs.recover_transaction()?;
        Ok(fs)
    }

    fn write_transaction(&mut self, txn: &FsTransaction) -> Result<(), FsBackendError> {
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + FS_SUPERBLOCK_LBA, &mut sector)?;
        let mut encoded=[0u8; TXN_SIZE];
        txn.encode(&mut encoded);
        sector[TXN_OFFSET..TXN_OFFSET + TXN_SIZE].copy_from_slice(&encoded);
        self.device.write_sector(self.base_lba + FS_SUPERBLOCK_LBA, &sector)?;
        Ok(())
    }

    fn clear_transaction(&mut self) -> Result<(), FsBackendError> {
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + FS_SUPERBLOCK_LBA, &mut sector)?;
        sector[TXN_OFFSET..TXN_OFFSET + TXN_SIZE].fill(0);
        self.device.write_sector(self.base_lba + FS_SUPERBLOCK_LBA, &sector)?;
        Ok(())
    }

    fn inode_matches(&mut self, index: u32, used: bool, size: u64, start: u32, count: u32) -> Result<bool, FsBackendError> {
        let inode = self.read_inode(index)?;
        Ok(inode.used == used && inode.file_size == size && inode.start_sector == start && inode.sector_count == count)
    }

    fn recover_transaction(&mut self) -> Result<(), FsBackendError> {
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + FS_SUPERBLOCK_LBA, &mut sector)?;
        let mut raw=[0u8; TXN_SIZE];
        raw.copy_from_slice(&sector[TXN_OFFSET..TXN_OFFSET + TXN_SIZE]);
        let Some(txn)=FsTransaction::decode(&raw) else { return Ok(()); };
        if txn.inode_index >= MAX_INODES { return Err(FsBackendError::CorruptMetadata); }

        // An in-place extension keeps the same extent, so there is nothing to
        // free: whichever size the inode carries is the file's size, and the
        // bytes past it are not part of the file.
        if txn.op == TxnOp::Extend as u8 {
            return self.clear_transaction();
        }

        let current_new = self.inode_matches(txn.inode_index, txn.new_used, txn.new_size, txn.new_start, txn.new_count)?;
        let current_old = self.inode_matches(txn.inode_index, txn.old_used, txn.old_size, txn.old_start, txn.old_count)?;
        if current_new {
            if txn.old_count != 0 { self.free_extent(txn.old_start, txn.old_count)?; }
        } else if current_old {
            if txn.new_count != 0 { self.free_extent(txn.new_start, txn.new_count)?; }
        } else {
            return Err(FsBackendError::CorruptMetadata);
        }
        self.clear_transaction()?;
        Ok(())
    }

    /// Sectors of data the filesystem can address.
    pub fn capacity_sectors(&self) -> u32 { self.superblock.layout.data_sectors }
    /// Bytes of data the filesystem can hold.
    pub fn capacity_bytes(&self) -> u64 { self.superblock.layout.capacity_bytes() }
    pub fn max_inodes(&self) -> u32 { self.superblock.inode_count }
    /// The geometry this filesystem was formatted with.
    pub fn layout(&self) -> FsLayout { self.superblock.layout }
    /// Sectors of the region this filesystem occupies.
    pub fn region_sectors(&self) -> u32 { self.superblock.total_sectors }

    fn read_inode(&mut self, index: u32) -> Result<DiskInode, FsBackendError> {
        if index >= MAX_INODES { return Err(FsBackendError::CorruptMetadata); }
        let lba=self.superblock.layout.inode_lba + index / INODES_PER_SECTOR as u32;
        let offset=(index as usize % INODES_PER_SECTOR) * INODE_SIZE;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba,&mut sector)?;
        let mut raw=[0u8; INODE_SIZE];
        raw.copy_from_slice(&sector[offset..offset+INODE_SIZE]);
        let inode = DiskInode::decode(&raw)?;
        if !inode.used {
            return Ok(inode);
        }

        let path = inode.path()?;
        if path.is_empty() || !path.starts_with('/') || path.contains("//") {
            return Err(FsBackendError::CorruptMetadata);
        }

        if inode.file_size == 0 {
            if inode.start_sector != 0 || inode.sector_count != 0 {
                return Err(FsBackendError::CorruptMetadata);
            }
        } else {
            if inode.sector_count == 0 || inode.start_sector.checked_add(inode.sector_count)
                .map_or(true, |end| end > self.superblock.layout.data_sectors)
            {
                return Err(FsBackendError::CorruptMetadata);
            }
            let capacity = (inode.sector_count as u64) * SECTOR_SIZE as u64;
            if inode.file_size > capacity {
                return Err(FsBackendError::CorruptMetadata);
            }
        }
        Ok(inode)
    }

    fn write_inode(&mut self, index: u32, inode: &DiskInode) -> Result<(), FsBackendError> {
        if index >= MAX_INODES { return Err(FsBackendError::CorruptMetadata); }
        let lba=self.superblock.layout.inode_lba + index / INODES_PER_SECTOR as u32;
        let offset=(index as usize % INODES_PER_SECTOR) * INODE_SIZE;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba,&mut sector)?;
        let mut raw=[0u8; INODE_SIZE];
        inode.encode(&mut raw);
        sector[offset..offset+INODE_SIZE].copy_from_slice(&raw);
        self.device.write_sector(self.base_lba + lba,&sector)?;
        Ok(())
    }

    /// Whether a data sector is allocated.
    #[allow(dead_code)]
    fn bitmap_bit(&mut self, block: u32) -> Result<bool, FsBackendError> {
        if block >= self.superblock.layout.data_sectors { return Err(FsBackendError::CorruptMetadata); }
        let byte_index=block as usize / 8;
        let bit=1u8 << (block % 8);
        let lba=self.superblock.layout.bitmap_lba + (byte_index / SECTOR_SIZE) as u32;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba, &mut sector)?;
        Ok((sector[byte_index % SECTOR_SIZE] & bit) != 0)
    }

    /// Allocates or releases one data sector in the bitmap.
    #[allow(dead_code)]
    fn set_bitmap_bit(&mut self, block: u32, used: bool) -> Result<(), FsBackendError> {
        if block >= self.superblock.layout.data_sectors { return Err(FsBackendError::CorruptMetadata); }
        let byte_index=block as usize / 8;
        let bit=1u8 << (block % 8);
        let lba=self.superblock.layout.bitmap_lba + (byte_index / SECTOR_SIZE) as u32;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba, &mut sector)?;
        if used { sector[byte_index % SECTOR_SIZE] |= bit; }
        else { sector[byte_index % SECTOR_SIZE] &= !bit; }
        self.device.write_sector(self.base_lba + lba, &sector)?;
        Ok(())
    }

    fn find_free_extent(&mut self, needed: u32) -> Result<u32, FsBackendError> {
        if needed == 0 { return Ok(0); }
        let max = self.superblock.layout.data_sectors;
        let mut run = 0u32;
        let mut start = 0u32;
        let mut cur_lba = u32::MAX;
        let mut cur_sector = [0u8; SECTOR_SIZE];
        for block in 0..max {
            let byte_index = block as usize / 8;
            let bit = 1u8 << (block % 8);
            let lba = self.superblock.layout.bitmap_lba + (byte_index / SECTOR_SIZE) as u32;
            if lba != cur_lba {
                self.device.read_sector(self.base_lba + lba, &mut cur_sector)?;
                cur_lba = lba;
            }
            let used = (cur_sector[byte_index % SECTOR_SIZE] & bit) != 0;
            if !used {
                if run == 0 { start = block; }
                run += 1;
                if run >= needed { return Ok(start); }
            } else {
                run = 0;
            }
        }
        Err(FsBackendError::NoSpace)
    }

    fn set_extent_bitmap(&mut self, start: u32, count: u32, used: bool) -> Result<(), FsBackendError> {
        if count == 0 { return Ok(()); }
        if start.checked_add(count).unwrap_or(u32::MAX) > self.superblock.layout.data_sectors {
            return Err(FsBackendError::CorruptMetadata);
        }
        let mut cur_lba = u32::MAX;
        let mut cur_sector = [0u8; SECTOR_SIZE];
        let mut dirty = false;
        for block in start..start + count {
            let byte_index = block as usize / 8;
            let bit = 1u8 << (block % 8);
            let lba = self.superblock.layout.bitmap_lba + (byte_index / SECTOR_SIZE) as u32;
            if lba != cur_lba {
                if dirty && cur_lba != u32::MAX {
                    self.device.write_sector(self.base_lba + cur_lba, &cur_sector)?;
                }
                self.device.read_sector(self.base_lba + lba, &mut cur_sector)?;
                cur_lba = lba;
            }
            if used {
                cur_sector[byte_index % SECTOR_SIZE] |= bit;
            } else {
                cur_sector[byte_index % SECTOR_SIZE] &= !bit;
            }
            dirty = true;
        }
        if dirty && cur_lba != u32::MAX {
            self.device.write_sector(self.base_lba + cur_lba, &cur_sector)?;
        }
        Ok(())
    }

    fn mark_extent(&mut self, start: u32, count: u32) -> Result<(), FsBackendError> {
        self.set_extent_bitmap(start, count, true)
    }

    fn free_extent(&mut self, start: u32, count: u32) -> Result<(), FsBackendError> {
        self.set_extent_bitmap(start, count, false)
    }

    fn find_inode(&mut self, path: &str) -> Result<Option<(u32, DiskInode)>, FsBackendError> {
        let mut cur_lba = u32::MAX;
        let mut cur_sector = [0u8; SECTOR_SIZE];
        for index in 0..MAX_INODES {
            let lba = self.superblock.layout.inode_lba + index / INODES_PER_SECTOR as u32;
            let offset = (index as usize % INODES_PER_SECTOR) * INODE_SIZE;
            if lba != cur_lba {
                self.device.read_sector(self.base_lba + lba, &mut cur_sector)?;
                cur_lba = lba;
            }
            let mut raw = [0u8; INODE_SIZE];
            raw.copy_from_slice(&cur_sector[offset..offset+INODE_SIZE]);
            let inode = DiskInode::decode(&raw)?;
            if inode.used && inode.path()? == path { return Ok(Some((index, inode))); }
        }
        Ok(None)
    }

    /// The first free inode index, or `TooManyFiles` when the table is full.
    fn alloc_inode_index(&mut self) -> Result<u32, FsBackendError> {
        let mut cur_lba = u32::MAX;
        let mut cur_sector = [0u8; SECTOR_SIZE];
        for index in 0..MAX_INODES {
            let lba = self.superblock.layout.inode_lba + index / INODES_PER_SECTOR as u32;
            let offset = (index as usize % INODES_PER_SECTOR) * INODE_SIZE;
            if lba != cur_lba {
                self.device.read_sector(self.base_lba + lba, &mut cur_sector)?;
                cur_lba = lba;
            }
            let mut raw = [0u8; INODE_SIZE];
            raw.copy_from_slice(&cur_sector[offset..offset+INODE_SIZE]);
            let inode = DiskInode::decode(&raw)?;
            if !inode.used { return Ok(index); }
        }
        Err(FsBackendError::TooManyFiles)
    }

    fn validate_path(path: &str) -> Result<(), FsBackendError> {
        if path.is_empty() || !path.starts_with('/') || path.len() > MAX_PATH || path.contains("//") {
            return Err(FsBackendError::InvalidPath);
        }
        Ok(())
    }

    /// Writes `data` into the file's reserved extent at `offset`.
    ///
    /// A sector that is only partly covered is read, patched and written back,
    /// because a streamed write lands on arbitrary byte offsets.
    fn write_extent_bytes(&mut self, inode: &DiskInode, offset: u64, data: &[u8]) -> Result<(), FsBackendError> {
        let mut written = 0usize;
        while written < data.len() {
            let absolute = offset + written as u64;
            let sector_index = (absolute / SECTOR_SIZE as u64) as u32;
            let within = (absolute % SECTOR_SIZE as u64) as usize;
            let take = core::cmp::min(SECTOR_SIZE - within, data.len() - written);
            let lba = self.base_lba + self.superblock.layout.data_start + inode.start_sector + sector_index;
            if within == 0 && take == SECTOR_SIZE {
                let mut sector = [0u8; SECTOR_SIZE];
                sector.copy_from_slice(&data[written..written + take]);
                self.device.write_sector(lba, &sector)?;
            } else {
                let mut sector = [0u8; SECTOR_SIZE];
                self.device.read_sector(lba, &mut sector)?;
                sector[within..within + take].copy_from_slice(&data[written..written + take]);
                self.device.write_sector(lba, &sector)?;
            }
            written += take;
        }
        Ok(())
    }

    /// Reads up to `out.len()` bytes from the file's extent at `offset`.
    fn read_extent_bytes(&mut self, inode: &DiskInode, offset: u64, out: &mut [u8]) -> Result<usize, FsBackendError> {
        let mut copied = 0usize;
        while copied < out.len() {
            let absolute = offset + copied as u64;
            let sector_index = (absolute / SECTOR_SIZE as u64) as u32;
            let within = (absolute % SECTOR_SIZE as u64) as usize;
            let take = core::cmp::min(SECTOR_SIZE - within, out.len() - copied);
            let lba = self.base_lba + self.superblock.layout.data_start + inode.start_sector + sector_index;
            let mut sector = [0u8; SECTOR_SIZE];
            self.device.read_sector(lba, &mut sector)?;
            out[copied..copied + take].copy_from_slice(&sector[within..within + take]);
            copied += take;
        }
        Ok(copied)
    }

    /// Moves a file's extent to a larger one, keeping the bytes it holds.
    ///
    /// WHY geometric growth: a streamed writer that does not know the final
    /// size would otherwise relocate the file on every chunk, and the copy
    /// would make writing a file quadratic in its size. Doubling keeps the
    /// number of relocations logarithmic and the total copy linear.
    fn grow_extent(&mut self, index: u32, old: &DiskInode, needed: u64) -> Result<DiskInode, FsBackendError> {
        let old_capacity = old.capacity();
        let target = core::cmp::max(needed, old_capacity.saturating_mul(2));
        let exact = sectors_for(needed).ok_or(FsBackendError::FileTooLarge)?;
        let room = self.superblock.layout.data_sectors;
        let mut chosen: Option<(u32, u32)> = None;
        if let Some(sectors) = sectors_for(target).filter(|s| *s <= room) {
            if let Ok(start) = self.find_free_extent(sectors) {
                chosen = Some((start, sectors));
            }
        }
        if chosen.is_none() {
            // Fragmentation must not stop a file that still fits: fall back to
            // exactly the size the caller asked for.
            let start = self.find_free_extent(exact)?;
            chosen = Some((start, exact));
        }
        let (start, sectors) = chosen.ok_or(FsBackendError::NoSpace)?;
        let mut inode = *old;
        inode.start_sector = start;
        inode.sector_count = sectors;
        self.relocate_extent(index, old, &inode)
    }

    /// Publishes a new extent for a file and releases the old one.
    fn relocate_extent(&mut self, index: u32, old: &DiskInode, new: &DiskInode) -> Result<DiskInode, FsBackendError> {
        let txn = FsTransaction {
            op: TxnOp::Write as u8,
            inode_index: index,
            old_used: old.used, old_size: old.file_size, old_start: old.start_sector, old_count: old.sector_count,
            new_used: new.used, new_size: new.file_size, new_start: new.start_sector, new_count: new.sector_count,
        };
        self.write_transaction(&txn)?;
        if let Err(e) = self.mark_extent(new.start_sector, new.sector_count) {
            let _ = self.clear_transaction();
            return Err(e);
        }
        // The old bytes are copied before the inode is published, so the file
        // never points at an extent that does not hold its content yet.
        if let Err(e) = self.copy_extent(old, new) {
            let _ = self.free_extent(new.start_sector, new.sector_count);
            let _ = self.clear_transaction();
            return Err(e);
        }
        if let Err(e) = self.write_inode(index, new) {
            let _ = self.free_extent(new.start_sector, new.sector_count);
            let _ = self.clear_transaction();
            return Err(e);
        }
        if old.sector_count != 0 {
            self.free_extent(old.start_sector, old.sector_count)?;
        }
        self.clear_transaction()?;
        Ok(*new)
    }

    /// Copies the bytes a file holds from one extent into another.
    fn copy_extent(&mut self, old: &DiskInode, new: &DiskInode) -> Result<(), FsBackendError> {
        if old.file_size == 0 { return Ok(()); }
        let mut copied = 0u64;
        while copied < old.file_size {
            let take = core::cmp::min(SECTOR_SIZE as u64, old.file_size - copied) as usize;
            let mut sector = [0u8; SECTOR_SIZE];
            let src = self.base_lba + self.superblock.layout.data_start + old.start_sector + (copied / SECTOR_SIZE as u64) as u32;
            let dst = self.base_lba + self.superblock.layout.data_start + new.start_sector + (copied / SECTOR_SIZE as u64) as u32;
            self.device.read_sector(src, &mut sector)?;
            self.device.write_sector(dst, &sector)?;
            copied += take as u64;
        }
        Ok(())
    }

    pub fn create_file(&mut self, path: &str, data: &[u8]) -> Result<u32, FsBackendError> {
        Self::validate_path(path)?;
        if self.find_inode(path)?.is_some() { return Err(FsBackendError::InvalidPath); }
        let data_len = data.len() as u64;
        if data_len > (self.superblock.layout.data_sectors as u64).saturating_mul(SECTOR_SIZE as u64) {
            return Err(FsBackendError::FileTooLarge);
        }
        let sectors=sectors_for(data_len).unwrap_or(0);
        let index=self.alloc_inode_index()?;
        let start=self.find_free_extent(sectors)?;
        let mut inode=DiskInode::empty();
        inode.used=true;
        inode.file_size=data_len;
        inode.start_sector=if sectors == 0 { 0 } else { start };
        inode.sector_count=sectors;
        inode.set_path(path);
        let txn = FsTransaction {
            op: TxnOp::Create as u8,
            inode_index: index,
            old_used: false, old_size: 0, old_start: 0, old_count: 0,
            new_used: true, new_size: inode.file_size, new_start: inode.start_sector, new_count: inode.sector_count,
        };
        if let Err(e)=self.write_transaction(&txn) {
            return Err(e);
        }
        if let Err(e)=self.mark_extent(start,sectors) {
            let _=self.free_extent(start,sectors);
            let _=self.clear_transaction();
            return Err(e);
        }
        for block in 0..sectors {
            let mut sector=[0u8; SECTOR_SIZE];
            let begin=block as usize*SECTOR_SIZE;
            let end=(begin+SECTOR_SIZE).min(data.len());
            if begin<end { sector[..end-begin].copy_from_slice(&data[begin..end]); }
            if let Err(e)=self.device.write_sector(self.base_lba + self.superblock.layout.data_start+start+block,&sector) {
                let _=self.free_extent(start,sectors);
                let _=self.clear_transaction();
                return Err(e.into());
            }
        }
        if let Err(e)=self.write_inode(index,&inode) {
            let _=self.free_extent(start,sectors);
            return Err(e);
        }
        self.clear_transaction()?;
        Ok(index)
    }

    /// Creates a file whose extent is reserved for `capacity` bytes.
    ///
    /// WHY a reserved extent rather than one that grows with the content: a
    /// streamed write must not relocate the file's blocks on every chunk. The
    /// reservation is made once, and each chunk is then an in-place sector
    /// write.
    pub fn create_stream(&mut self, path: &str, capacity: u64) -> Result<u32, FsBackendError> {
        Self::validate_path(path)?;
        if self.find_inode(path)?.is_some() { return Err(FsBackendError::InvalidPath); }
        let sectors = sectors_for(capacity).ok_or(FsBackendError::FileTooLarge)?;
        if sectors > self.superblock.layout.data_sectors {
            return Err(FsBackendError::FileTooLarge);
        }
        let index = self.alloc_inode_index()?;
        let start = self.find_free_extent(sectors)?;
        let mut inode = DiskInode::empty();
        inode.used = true;
        inode.file_size = 0;
        inode.start_sector = if sectors == 0 { 0 } else { start };
        inode.sector_count = sectors;
        inode.set_path(path);
        let txn = FsTransaction {
            op: TxnOp::Create as u8,
            inode_index: index,
            old_used: false, old_size: 0, old_start: 0, old_count: 0,
            new_used: true, new_size: inode.file_size, new_start: inode.start_sector, new_count: inode.sector_count,
        };
        self.write_transaction(&txn)?;
        if let Err(e) = self.mark_extent(start, sectors) {
            let _ = self.free_extent(start, sectors);
            let _ = self.clear_transaction();
            return Err(e);
        }
        if let Err(e) = self.write_inode(index, &inode) {
            let _ = self.free_extent(start, sectors);
            return Err(e);
        }
        self.clear_transaction()?;
        Ok(index)
    }

    /// The published size of a file, in bytes.
    pub fn file_len(&mut self, path: &str) -> Result<u64, FsBackendError> {
        Self::validate_path(path)?;
        let (_index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        Ok(inode.file_size)
    }

    /// Makes sure a file exists with room for `capacity` bytes.
    ///
    /// An existing file keeps its bytes and is grown if it does not have the
    /// room; a missing one is created with a reserved extent.
    pub fn reserve_stream(&mut self, path: &str, capacity: u64) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        match self.find_inode(path)? {
            Some((index, inode)) => {
                if inode.capacity() >= capacity { return Ok(()); }
                self.grow_extent(index, &inode, capacity)?;
                Ok(())
            }
            None => {
                self.create_stream(path, capacity)?;
                Ok(())
            }
        }
    }

    /// Writes `data` at `offset` inside a file, growing the file if needed.
    ///
    /// WHY the size is published separately from the bytes: the size is the
    /// file's commit point. A crash between the byte write and the size
    /// publication leaves the file at its old size, and the bytes past it are
    /// not part of the file.
    pub fn write_at(&mut self, path: &str, offset: u64, data: &[u8]) -> Result<usize, FsBackendError> {
        Self::validate_path(path)?;
        if data.is_empty() { return Ok(0); }
        let (index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        let end = offset.checked_add(data.len() as u64).ok_or(FsBackendError::FileTooLarge)?;
        let mut current = inode;
        if end > current.capacity() {
            current = self.grow_extent(index, &current, end)?;
        }
        self.write_extent_bytes(&current, offset, data)?;
        if end > current.file_size {
            let txn = FsTransaction {
                op: TxnOp::Extend as u8,
                inode_index: index,
                old_used: current.used, old_size: current.file_size, old_start: current.start_sector, old_count: current.sector_count,
                new_used: current.used, new_size: end, new_start: current.start_sector, new_count: current.sector_count,
            };
            self.write_transaction(&txn)?;
            let mut updated = current;
            updated.file_size = end;
            if let Err(e) = self.write_inode(index, &updated) {
                let _ = self.clear_transaction();
                return Err(e);
            }
            self.clear_transaction()?;
        }
        Ok(data.len())
    }

    /// Reads up to `out.len()` bytes from a file at `offset`.
    pub fn read_at(&mut self, path: &str, offset: u64, out: &mut [u8]) -> Result<usize, FsBackendError> {
        Self::validate_path(path)?;
        let (_index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        if offset >= inode.file_size || out.is_empty() { return Ok(0); }
        let available = (inode.file_size - offset) as usize;
        let take = core::cmp::min(available, out.len());
        self.read_extent_bytes(&inode, offset, &mut out[..take])
    }

    pub fn read_file(&mut self, path: &str, out: &mut [u8]) -> Result<usize, FsBackendError> {
        Self::validate_path(path)?;
        let (_index,inode)=self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        if inode.file_size == 0 { return Ok(0); }
        if inode.sector_count == 0 || inode.start_sector.checked_add(inode.sector_count).unwrap_or(u32::MAX) > self.superblock.layout.data_sectors {
            return Err(FsBackendError::CorruptMetadata);
        }
        if out.len() < inode.file_size as usize { return Err(FsBackendError::FileTooLarge); }
        let mut copied=0usize;
        for block in 0..inode.sector_count {
            let mut sector=[0u8; SECTOR_SIZE];
            self.device.read_sector(self.base_lba + self.superblock.layout.data_start+inode.start_sector+block,&mut sector)?;
            let take=(inode.file_size as usize-copied).min(SECTOR_SIZE);
            if take>0 { out[copied..copied+take].copy_from_slice(&sector[..take]); copied+=take; }
            if copied>=inode.file_size as usize { break; }
        }
        Ok(copied)
    }

    /// Replaces file contents through the atomic transaction protocol: the intent
    /// is durable before extent reservation, payload is written before inode
    /// publication, and recovery finalizes or rolls back an interrupted update.
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        let (index,old)=self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        let data_len = data.len() as u64;
        if data_len > (self.superblock.layout.data_sectors as u64).saturating_mul(SECTOR_SIZE as u64) {
            return Err(FsBackendError::FileTooLarge);
        }
        let sectors=sectors_for(data_len).unwrap_or(0);
        let start=self.find_free_extent(sectors)?;
        let mut inode=old;
        inode.file_size=data_len;
        inode.start_sector=if sectors == 0 { 0 } else { start };
        inode.sector_count=sectors;
        let txn = FsTransaction {
            op: TxnOp::Write as u8,
            inode_index: index,
            old_used: old.used, old_size: old.file_size, old_start: old.start_sector, old_count: old.sector_count,
            new_used: inode.used, new_size: inode.file_size, new_start: inode.start_sector, new_count: inode.sector_count,
        };
        self.write_transaction(&txn)?;
        if let Err(e)=self.mark_extent(start,sectors) {
            let _=self.clear_transaction();
            return Err(e);
        }
        for block in 0..sectors {
            let mut sector=[0u8; SECTOR_SIZE];
            let begin=block as usize*SECTOR_SIZE;
            let end=(begin+SECTOR_SIZE).min(data.len());
            if begin<end { sector[..end-begin].copy_from_slice(&data[begin..end]); }
            if let Err(e)=self.device.write_sector(self.base_lba + self.superblock.layout.data_start+start+block,&sector) {
                let _=self.free_extent(start,sectors);
                let _=self.clear_transaction();
                return Err(e.into());
            }
        }
        if let Err(e)=self.write_inode(index,&inode) {
            let _=self.free_extent(start,sectors);
            return Err(e);
        }
        self.free_extent(old.start_sector,old.sector_count)?;
        self.clear_transaction()?;
        Ok(())
    }

    /// Resizes a file while preserving existing bytes and zero-filling growth.
    ///
    /// The final publication uses the existing write transaction protocol, so a
    /// crash cannot expose a partially rewritten inode or leaked extent.
    ///
    /// WHY it does not rewrite the whole file: the old implementation read the
    /// file into a buffer and wrote it back, which needs as much memory as the
    /// file is large. A 2 GiB model cannot be resized that way on a 64 MiB
    /// heap, so the extent is resized in place instead.
    pub fn truncate_file(&mut self, path: &str, new_size: u64) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        let (index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        if new_size == inode.file_size {
            return Ok(());
        }
        if new_size > (self.superblock.layout.data_sectors as u64).saturating_mul(SECTOR_SIZE as u64) {
            return Err(FsBackendError::FileTooLarge);
        }
        let mut current = inode;
        if new_size > current.capacity() {
            current = self.grow_extent(index, &current, new_size)?;
        }
        if new_size > current.file_size {
            // Zero-fill the bytes between the old size and the new one.
            let start = current.file_size;
            let mut remaining = new_size - start;
            let mut offset = start;
            let zeros = [0u8; SECTOR_SIZE];
            while remaining > 0 {
                let take = core::cmp::min(SECTOR_SIZE as u64, remaining) as usize;
                self.write_extent_bytes(&current, offset, &zeros[..take])?;
                offset += take as u64;
                remaining -= take as u64;
            }
        }
        let txn = FsTransaction {
            op: TxnOp::Extend as u8,
            inode_index: index,
            old_used: current.used, old_size: current.file_size, old_start: current.start_sector, old_count: current.sector_count,
            new_used: current.used, new_size: new_size, new_start: current.start_sector, new_count: current.sector_count,
        };
        self.write_transaction(&txn)?;
        let mut updated = current;
        updated.file_size = new_size;
        if let Err(e) = self.write_inode(index, &updated) {
            let _ = self.clear_transaction();
            return Err(e);
        }
        self.clear_transaction()?;
        Ok(())
    }

    pub fn check_integrity(&mut self) -> Result<FsCheckReport, FsBackendError> {
        let mut report = FsCheckReport { used_inodes: 0, allocated_sectors: 0, referenced_sectors: 0 };
        // One bit per data sector: a 6 GiB region needs 786 KiB of reference
        // map, which is affordable, and it is what makes the comparison
        // against the on-disk bitmap exact.
        let mut referenced = alloc::vec![false; self.superblock.layout.data_sectors as usize];
        let mut cur_inode_lba = u32::MAX;
        let mut cur_inode_sector = [0u8; SECTOR_SIZE];
        for index in 0..MAX_INODES {
            let lba = self.superblock.layout.inode_lba + index / INODES_PER_SECTOR as u32;
            let offset = (index as usize % INODES_PER_SECTOR) * INODE_SIZE;
            if lba != cur_inode_lba {
                self.device.read_sector(self.base_lba + lba, &mut cur_inode_sector)?;
                cur_inode_lba = lba;
            }
            let mut raw = [0u8; INODE_SIZE];
            raw.copy_from_slice(&cur_inode_sector[offset..offset+INODE_SIZE]);
            let inode = DiskInode::decode(&raw)?;
            if !inode.used { continue; }
            report.used_inodes = report.used_inodes.saturating_add(1);
            if inode.path_len == 0 || inode.path_len as usize > MAX_PATH { return Err(FsBackendError::CorruptMetadata); }
            let _ = inode.path()?;
            if inode.file_size == 0 {
                if inode.sector_count != 0 { return Err(FsBackendError::CorruptMetadata); }
                continue;
            }
            let end = inode.start_sector.checked_add(inode.sector_count).ok_or(FsBackendError::CorruptMetadata)?;
            if inode.sector_count == 0 || end > self.superblock.layout.data_sectors { return Err(FsBackendError::CorruptMetadata); }
            let required = ((inode.file_size + SECTOR_SIZE as u64 - 1) / SECTOR_SIZE as u64) as u32;
            // The extent may be larger than the file needs: a streamed file
            // reserves its blocks before it fills them.
            if inode.sector_count < required { return Err(FsBackendError::CorruptMetadata); }
            for block in inode.start_sector..end {
                let slot = &mut referenced[block as usize];
                if *slot { return Err(FsBackendError::CorruptMetadata); }
                *slot = true;
                report.referenced_sectors = report.referenced_sectors.saturating_add(1);
            }
        }
        let mut cur_lba = u32::MAX;
        let mut cur_sector = [0u8; SECTOR_SIZE];
        for block in 0..self.superblock.layout.data_sectors {
            let byte_index = block as usize / 8;
            let bit = 1u8 << (block % 8);
            let lba = self.superblock.layout.bitmap_lba + (byte_index / SECTOR_SIZE) as u32;
            if lba != cur_lba {
                self.device.read_sector(self.base_lba + lba, &mut cur_sector)?;
                cur_lba = lba;
            }
            let marked = (cur_sector[byte_index % SECTOR_SIZE] & bit) != 0;
            if marked != referenced[block as usize] { return Err(FsBackendError::CorruptMetadata); }
            if marked { report.allocated_sectors = report.allocated_sectors.saturating_add(1); }
        }
        Ok(report)
    }

    pub fn delete_file(&mut self, path: &str) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        let (index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        let txn = FsTransaction {
            op: TxnOp::Delete as u8,
            inode_index: index,
            old_used: inode.used, old_size: inode.file_size, old_start: inode.start_sector, old_count: inode.sector_count,
            new_used: false, new_size: 0, new_start: 0, new_count: 0,
        };
        self.write_transaction(&txn)?;
        self.write_inode(index, &DiskInode::empty())?;
        if inode.sector_count != 0 {
            self.free_extent(inode.start_sector, inode.sector_count)?;
        }
        self.clear_transaction()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    /// A device that only stores the sectors that were written.
    ///
    /// WHY it is sparse: the geometry tests format a 6 GiB region, and a
    /// device that held every sector of that region in memory would need six
    /// gigabytes of host memory to prove a number.
    struct SparseDevice {
        total: u32,
        sectors: BTreeMap<u32, [u8; SECTOR_SIZE]>,
    }

    impl SparseDevice {
        fn new(total: u32) -> Self {
            Self { total, sectors: BTreeMap::new() }
        }
    }

    impl BlockDevice for SparseDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            if let Some(sector) = self.sectors.get(&lba) {
                buf.copy_from_slice(sector);
            } else {
                buf.fill(0);
            }
            Ok(())
        }
        fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            self.sectors.insert(lba, *buf);
            Ok(())
        }
        fn sector_count(&self) -> u32 { self.total }
        fn model(&self) -> &str { "sparse" }
    }

    /// 6 GiB in sectors.
    ///
    /// WHY the multiplication is written the way it is: 6 * 1024 * 1024 * 1024
    /// overflows a `u32` before the division by the sector size, and a
    /// constant that overflows is an error rather than a wrong number.
    const SIX_GB_SECTORS: u32 = 6 * 1024 * 1024 * 2;
    /// 512 MiB in sectors.
    const HALF_GB_SECTORS: u32 = 512 * 1024 * 1024 / 512;

    /// The defect this pins: the geometry used to be a constant, so every
    /// filesystem was 16.8 MiB of usable data however large the disk was.
    ///
    /// The proving assertions are the capacities: a 6 GiB disk has to address
    /// far more than the old 32,768 sectors, and it has to address more than
    /// a 512 MiB disk does.
    #[test]
    fn the_usable_area_follows_the_size_of_the_disk() {
        let mut small = SparseDevice::new(HALF_GB_SECTORS);
        let mut large = SparseDevice::new(SIX_GB_SECTORS);

        let small_fs = InterFs::format_at(&mut small, 0).expect("a 512 MiB disk must format");
        let large_fs = InterFs::format_at(&mut large, 0).expect("a 6 GiB disk must format");

        assert!(
            large_fs.capacity_bytes() > 5 * 1024 * 1024 * 1024,
            "a 6 GiB disk must address gigabytes, not {} bytes",
            large_fs.capacity_bytes(),
        );
        assert!(
            large_fs.capacity_bytes() > small_fs.capacity_bytes() * 10,
            "a 6 GiB disk must address far more than a 512 MiB disk: {} vs {}",
            large_fs.capacity_bytes(),
            small_fs.capacity_bytes(),
        );
        // The old layout could address 32,768 sectors no matter what.
        assert!(large_fs.capacity_sectors() > 32_768);
    }

    /// The bitmap has to grow with the region, or the data region cannot be
    /// addressed at all.
    #[test]
    fn the_bitmap_covers_the_whole_data_region() {
        for total in [HALF_GB_SECTORS, SIX_GB_SECTORS] {
            let layout = FsLayout::for_region(total).expect("a usable region");
            let bitmap_capacity = layout.bitmap_sectors as u64 * SECTOR_SIZE as u64 * 8;
            assert!(
                bitmap_capacity >= layout.data_sectors as u64,
                "the bitmap must address every data sector: {} bits for {} sectors",
                bitmap_capacity,
                layout.data_sectors,
            );
        }
    }

    /// A superblock written by the old layout must not be read as this one.
    ///
    /// WHY the version field is the whole defence: the two layouts disagree
    /// about what the bytes after the magic mean, so a version 1 superblock
    /// read as version 2 would place the data region somewhere the old format
    /// never wrote, and every file would come back as somebody else's bytes.
    #[test]
    fn a_superblock_from_the_old_layout_is_rejected_rather_than_misread() {
        // A version 1 superblock: the same magic, version 1, and the old
        // fixed geometry (bitmap at LBA 1 with 8 sectors, inode table at 9,
        // data at 41).
        let mut old = [0u8; SECTOR_SIZE];
        old[0..4].copy_from_slice(&FS_MAGIC);
        old[4..8].copy_from_slice(&1u32.to_le_bytes());
        old[8..12].copy_from_slice(&HALF_GB_SECTORS.to_le_bytes());
        old[12..16].copy_from_slice(&41u32.to_le_bytes());
        old[16..20].copy_from_slice(&(HALF_GB_SECTORS - 41).to_le_bytes());
        old[20..24].copy_from_slice(&MAX_INODES.to_le_bytes());
        old[24..28].copy_from_slice(&1u32.to_le_bytes());
        old[28..32].copy_from_slice(&8u32.to_le_bytes());
        old[32..36].copy_from_slice(&9u32.to_le_bytes());
        old[36..40].copy_from_slice(&32u32.to_le_bytes());

        assert!(
            matches!(Superblock::decode(&old), Err(FsBackendError::UnsupportedVersion)),
            "a version 1 superblock must be refused, not reinterpreted",
        );

        // And a device that carries one does not mount.
        let mut dev = SparseDevice::new(HALF_GB_SECTORS);
        dev.write_sector(0, &old).expect("the superblock must be writable");
        assert_eq!(
            InterFs::mount_at(&mut dev, 0).err(),
            Some(FsBackendError::UnsupportedVersion),
        );
    }

    /// The other direction: a version 2 superblock is not a version 1 one, and
    /// the version field is what says so.
    #[test]
    fn a_superblock_from_this_layout_is_not_a_version_1_one() {
        let mut dev = SparseDevice::new(HALF_GB_SECTORS);
        // WHY the filesystem is scoped: it holds a borrow of the device, so
        // the sector read below has to happen after the filesystem is done
        // with it.
        let capacity = {
            let fs = InterFs::format_at(&mut dev, 0).expect("format");
            fs.capacity_sectors()
        };
        let mut sector = [0u8; SECTOR_SIZE];
        dev.read_sector(0, &mut sector).expect("read");
        assert_eq!(&sector[0..4], &FS_MAGIC);
        assert_eq!(u32::from_le_bytes([sector[4],sector[5],sector[6],sector[7]]), FS_VERSION);
        assert_ne!(FS_VERSION, 1);
        assert!(capacity > 32_768);
    }

    /// A file written a chunk at a time has to come back byte for byte, and
    /// the file's size has to be the size of what was written.
    #[test]
    fn a_file_written_in_chunks_reads_back_exactly() {
        let mut dev = SparseDevice::new(HALF_GB_SECTORS);
        InterFs::format_at(&mut dev, 0).expect("format");
        let mut fs = InterFs::mount_at(&mut dev, 0).expect("mount");

        fs.create_stream("/models/model.bin", 64 * 1024).expect("reserve the extent");

        let chunk: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let mut expected = Vec::new();
        for round in 0..5u32 {
            fs.write_at("/models/model.bin", round as u64 * 4096, &chunk).expect("write the chunk");
            expected.extend_from_slice(&chunk);
        }

        assert_eq!(fs.file_len("/models/model.bin").expect("length"), expected.len() as u64);

        let mut out = alloc::vec![0u8; expected.len()];
        let read = fs.read_at("/models/model.bin", 0, &mut out).expect("read back");
        assert_eq!(read, expected.len());
        assert_eq!(&out[..], &expected[..]);

        // ...and a read from the middle of the file is just as exact.
        let mut middle = [0u8; 100];
        let read = fs.read_at("/models/model.bin", 4096, &mut middle).expect("read the middle");
        assert_eq!(read, 100);
        assert_eq!(&middle[..], &expected[4096..4196]);
    }

    /// A file larger than the region cannot be reserved, and the refusal has
    /// to arrive before anything is allocated.
    #[test]
    fn a_file_larger_than_the_disk_is_refused() {
        let mut dev = SparseDevice::new(HALF_GB_SECTORS);
        InterFs::format_at(&mut dev, 0).expect("format");
        let mut fs = InterFs::mount_at(&mut dev, 0).expect("mount");
        assert_eq!(
            fs.create_stream("/models/too-big.bin", 4 * 1024 * 1024 * 1024).err(),
            Some(FsBackendError::FileTooLarge),
        );
    }

    /// The ordinary whole-file path still works, and the integrity checker
    /// still agrees with the bitmap.
    #[test]
    fn a_whole_file_round_trips_and_the_bitmap_agrees() {
        let mut dev = SparseDevice::new(HALF_GB_SECTORS);
        InterFs::format_at(&mut dev, 0).expect("format");
        let mut fs = InterFs::mount_at(&mut dev, 0).expect("mount");

        fs.create_file("/data/hello.txt", b"hello interfs").expect("create");
        let mut out = [0u8; 64];
        let read = fs.read_file("/data/hello.txt", &mut out).expect("read");
        assert_eq!(&out[..read], b"hello interfs");

        fs.write_file("/data/hello.txt", b"a different payload").expect("write");
        let read = fs.read_file("/data/hello.txt", &mut out).expect("read");
        assert_eq!(&out[..read], b"a different payload");

        let report = fs.check_integrity().expect("the filesystem must be consistent");
        assert_eq!(report.used_inodes, 1);
        assert!(report.allocated_sectors >= 1);

        fs.delete_file("/data/hello.txt").expect("delete");
        let report = fs.check_integrity().expect("the filesystem must be consistent");
        assert_eq!(report.used_inodes, 0);
        assert_eq!(report.allocated_sectors, 0);
    }

    /// The size of a file is what a reader sees, and it survives a remount.
    #[test]
    fn the_published_size_survives_a_remount() {
        let mut dev = SparseDevice::new(HALF_GB_SECTORS);
        InterFs::format_at(&mut dev, 0).expect("format");
        {
            let mut fs = InterFs::mount_at(&mut dev, 0).expect("mount");
            fs.create_stream("/models/sized.bin", 8192).expect("reserve");
            fs.write_at("/models/sized.bin", 0, &[7u8; 3000]).expect("write");
        }
        let mut fs = InterFs::mount_at(&mut dev, 0).expect("remount");
        assert_eq!(fs.file_len("/models/sized.bin").expect("length"), 3000);
        let mut out = [0u8; 3000];
        let read = fs.read_at("/models/sized.bin", 0, &mut out).expect("read");
        assert_eq!(read, 3000);
        assert!(out.iter().all(|b| *b == 7));
    }
}
