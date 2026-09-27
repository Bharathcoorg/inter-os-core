//! Minimal on-disk InterOS filesystem backend.
//!
//! Block-backed filesystem boundary above `BlockDevice`.
//! The format is intentionally small and deterministic: a superblock, allocation
//! bitmap, fixed-size inode table, and contiguous file data extents. VFS can use
//! this backend without knowing the physical ATA/SDHCI implementation,
//! with transactional metadata publication and deterministic crash
//! recovery around mutations.

use crate::crypto::Sha256;
use crate::drivers::{BlockDevice, AtaError, SECTOR_SIZE};

pub const FS_MAGIC: [u8; 4] = *b"IOFS";
pub const FS_VERSION: u32 = 1;
pub const FS_SUPERBLOCK_LBA: u32 = 0;
const BITMAP_LBA: u32 = 1;
const BITMAP_SECTORS: u32 = 8;
const INODE_LBA: u32 = BITMAP_LBA + BITMAP_SECTORS;
const INODE_SIZE: usize = 64;
const INODES_PER_SECTOR: usize = SECTOR_SIZE / INODE_SIZE;
const INODE_SECTORS: u32 = 32;
const DATA_LBA: u32 = INODE_LBA + INODE_SECTORS;
const MAX_INODES: u32 = (INODE_SECTORS as usize * INODES_PER_SECTOR) as u32;
const MAX_PATH: usize = 47;
const BITMAP_CAPACITY_SECTORS: u32 = BITMAP_SECTORS * SECTOR_SIZE as u32 * 8;
const TXN_OFFSET: usize = 64;
const TXN_SIZE: usize = 64;
const TXN_MAGIC: [u8; 4] = *b"IFTX";
const TXN_VERSION: u8 = 1;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxnOp { Create = 1, Write = 2, Delete = 3 }

#[derive(Debug, Clone, Copy)]
struct FsTransaction {
    op: u8,
    inode_index: u32,
    old_used: bool,
    old_size: u32,
    old_start: u32,
    old_count: u32,
    new_used: bool,
    new_size: u32,
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
        out[13..17].copy_from_slice(&self.old_size.to_le_bytes());
        out[17..21].copy_from_slice(&self.old_start.to_le_bytes());
        out[21..25].copy_from_slice(&self.old_count.to_le_bytes());
        out[25] = self.new_used as u8;
        out[26..30].copy_from_slice(&self.new_size.to_le_bytes());
        out[30..34].copy_from_slice(&self.new_start.to_le_bytes());
        out[34..38].copy_from_slice(&self.new_count.to_le_bytes());
        let digest = Sha256::digest(&out[..56]);
        out[56..64].copy_from_slice(&digest[..8]);
    }

    fn decode(buf: &[u8; TXN_SIZE]) -> Option<Self> {
        if buf[0..4] != TXN_MAGIC || buf[4] != TXN_VERSION { return None; }
        if Sha256::digest(&buf[..56])[..8] != buf[56..64] { return None; }
        if buf[12] > 1 || buf[25] > 1 { return None; }
        Some(Self {
            op: buf[5],
            inode_index: u32::from_le_bytes([buf[8],buf[9],buf[10],buf[11]]),
            old_used: buf[12] != 0,
            old_size: u32::from_le_bytes([buf[13],buf[14],buf[15],buf[16]]),
            old_start: u32::from_le_bytes([buf[17],buf[18],buf[19],buf[20]]),
            old_count: u32::from_le_bytes([buf[21],buf[22],buf[23],buf[24]]),
            new_used: buf[25] != 0,
            new_size: u32::from_le_bytes([buf[26],buf[27],buf[28],buf[29]]),
            new_start: u32::from_le_bytes([buf[30],buf[31],buf[32],buf[33]]),
            new_count: u32::from_le_bytes([buf[34],buf[35],buf[36],buf[37]]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsBackendError {
    Io(AtaError),
    InvalidFormat,
    NoSpace,
    TooManyFiles,
    FileNotFound,
    InvalidPath,
    FileTooLarge,
    CorruptMetadata,
}

impl From<AtaError> for FsBackendError {
    fn from(value: AtaError) -> Self { Self::Io(value) }
}

#[derive(Debug, Clone, Copy)]
struct Superblock {
    total_sectors: u32,
    data_start: u32,
    data_sectors: u32,
    inode_count: u32,
}

impl Superblock {
    fn encode(&self, out: &mut [u8; SECTOR_SIZE]) {
        out.fill(0);
        out[0..4].copy_from_slice(&FS_MAGIC);
        out[4..8].copy_from_slice(&FS_VERSION.to_le_bytes());
        out[8..12].copy_from_slice(&self.total_sectors.to_le_bytes());
        out[12..16].copy_from_slice(&self.data_start.to_le_bytes());
        out[16..20].copy_from_slice(&self.data_sectors.to_le_bytes());
        out[20..24].copy_from_slice(&self.inode_count.to_le_bytes());
        out[24..28].copy_from_slice(&BITMAP_LBA.to_le_bytes());
        out[28..32].copy_from_slice(&BITMAP_SECTORS.to_le_bytes());
        out[32..36].copy_from_slice(&INODE_LBA.to_le_bytes());
        out[36..40].copy_from_slice(&INODE_SECTORS.to_le_bytes());
    }

    fn decode(buf: &[u8; SECTOR_SIZE]) -> Result<Self, FsBackendError> {
        if buf[0..4] != FS_MAGIC
            || u32::from_le_bytes([buf[4],buf[5],buf[6],buf[7]]) != FS_VERSION {
            return Err(FsBackendError::InvalidFormat);
        }
        let total = u32::from_le_bytes([buf[8],buf[9],buf[10],buf[11]]);
        let data_start = u32::from_le_bytes([buf[12],buf[13],buf[14],buf[15]]);
        let data_sectors = u32::from_le_bytes([buf[16],buf[17],buf[18],buf[19]]);
        let inode_count = u32::from_le_bytes([buf[20],buf[21],buf[22],buf[23]]);
        if inode_count != MAX_INODES || data_start != DATA_LBA || data_sectors == 0 || data_sectors > BITMAP_CAPACITY_SECTORS
            || data_start.checked_add(data_sectors).unwrap_or(u32::MAX) > total {
            return Err(FsBackendError::CorruptMetadata);
        }
        Ok(Self { total_sectors: total, data_start, data_sectors, inode_count })
    }
}

#[derive(Debug, Clone, Copy)]
struct DiskInode {
    used: bool,
    file_size: u32,
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
        out[1..5].copy_from_slice(&self.file_size.to_le_bytes());
        out[5..9].copy_from_slice(&self.start_sector.to_le_bytes());
        out[9..13].copy_from_slice(&self.sector_count.to_le_bytes());
        out[13] = self.path_len;
        out[14..14+MAX_PATH].copy_from_slice(&self.path);
    }

    fn decode(buf: &[u8; INODE_SIZE]) -> Result<Self, FsBackendError> {
        if buf[0] > 1 || buf[13] as usize > MAX_PATH {
            return Err(FsBackendError::CorruptMetadata);
        }
        let mut path=[0u8; MAX_PATH];
        path.copy_from_slice(&buf[14..14+MAX_PATH]);
        Ok(Self {
            used: buf[0] != 0,
            file_size: u32::from_le_bytes([buf[1],buf[2],buf[3],buf[4]]),
            start_sector: u32::from_le_bytes([buf[5],buf[6],buf[7],buf[8]]),
            sector_count: u32::from_le_bytes([buf[9],buf[10],buf[11],buf[12]]),
            path_len: buf[13],
            path,
        })
    }

    fn path(&self) -> Result<&str, FsBackendError> {
        core::str::from_utf8(&self.path[..self.path_len as usize])
            .map_err(|_| FsBackendError::CorruptMetadata)
    }
}

pub struct InterFs<'a, D: BlockDevice> {
    device: &'a mut D,
    superblock: Superblock,
    base_lba: u32,
}

impl<'a, D: BlockDevice> InterFs<'a, D> {
    pub fn format(device: &'a mut D) -> Result<Self, FsBackendError> {
        Self::format_at(device, 0)
    }

    pub fn format_at(device: &'a mut D, base_lba: u32) -> Result<Self, FsBackendError> {
        let total = device.sector_count();
        let region_sectors = total.checked_sub(base_lba).ok_or(FsBackendError::NoSpace)?;
        Self::format_region(device, base_lba, region_sectors)
    }

    pub fn format_region(device: &'a mut D, base_lba: u32, region_sectors: u32) -> Result<Self, FsBackendError> {
        if region_sectors <= DATA_LBA + 1 || region_sectors - DATA_LBA > BITMAP_CAPACITY_SECTORS {
            return Err(FsBackendError::NoSpace);
        }
        let sb = Superblock {
            total_sectors: region_sectors,
            data_start: DATA_LBA,
            data_sectors: region_sectors - DATA_LBA,
            inode_count: MAX_INODES,
        };
        let mut sector=[0u8; SECTOR_SIZE];
        sb.encode(&mut sector);
        device.write_sector(base_lba + FS_SUPERBLOCK_LBA, &sector)?;

        sector.fill(0);
        for lba in BITMAP_LBA..BITMAP_LBA + BITMAP_SECTORS {
            device.write_sector(base_lba + lba, &sector)?;
        }
        for lba in INODE_LBA..INODE_LBA + INODE_SECTORS {
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
        let mut sector=[0u8; SECTOR_SIZE];
        device.read_sector(base_lba + FS_SUPERBLOCK_LBA, &mut sector)?;
        let sb=Superblock::decode(&sector)?;
        if sb.total_sectors != region_sectors {
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

    fn inode_matches(&mut self, index: u32, used: bool, size: u32, start: u32, count: u32) -> Result<bool, FsBackendError> {
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

    pub fn capacity_sectors(&self) -> u32 { self.superblock.data_sectors }
    pub fn max_inodes(&self) -> u32 { self.superblock.inode_count }

    fn read_inode(&mut self, index: u32) -> Result<DiskInode, FsBackendError> {
        if index >= MAX_INODES { return Err(FsBackendError::CorruptMetadata); }
        let lba=INODE_LBA + index / INODES_PER_SECTOR as u32;
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
                .map_or(true, |end| end > self.superblock.data_sectors)
            {
                return Err(FsBackendError::CorruptMetadata);
            }
            let capacity = (inode.sector_count as u64) * SECTOR_SIZE as u64;
            if inode.file_size as u64 > capacity {
                return Err(FsBackendError::CorruptMetadata);
            }
        }
        Ok(inode)
    }

    fn write_inode(&mut self, index: u32, inode: &DiskInode) -> Result<(), FsBackendError> {
        if index >= MAX_INODES { return Err(FsBackendError::CorruptMetadata); }
        let lba=INODE_LBA + index / INODES_PER_SECTOR as u32;
        let offset=(index as usize % INODES_PER_SECTOR) * INODE_SIZE;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba,&mut sector)?;
        let mut raw=[0u8; INODE_SIZE];
        inode.encode(&mut raw);
        sector[offset..offset+INODE_SIZE].copy_from_slice(&raw);
        self.device.write_sector(self.base_lba + lba,&sector)?;
        Ok(())
    }

    fn bitmap_bit(&mut self, block: u32) -> Result<bool, FsBackendError> {
        if block >= self.superblock.data_sectors { return Err(FsBackendError::CorruptMetadata); }
        let byte_index=block as usize / 8;
        let bit=1u8 << (block % 8);
        let lba=BITMAP_LBA + (byte_index / SECTOR_SIZE) as u32;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba,&mut sector)?;
        Ok((sector[byte_index % SECTOR_SIZE] & bit) != 0)
    }

    fn set_bitmap_bit(&mut self, block: u32, used: bool) -> Result<(), FsBackendError> {
        if block >= self.superblock.data_sectors { return Err(FsBackendError::CorruptMetadata); }
        let byte_index=block as usize / 8;
        let bit=1u8 << (block % 8);
        let lba=BITMAP_LBA + (byte_index / SECTOR_SIZE) as u32;
        let mut sector=[0u8; SECTOR_SIZE];
        self.device.read_sector(self.base_lba + lba,&mut sector)?;
        if used { sector[byte_index % SECTOR_SIZE] |= bit; }
        else { sector[byte_index % SECTOR_SIZE] &= !bit; }
        self.device.write_sector(self.base_lba + lba,&sector)?;
        Ok(())
    }

    fn find_free_extent(&mut self, needed: u32) -> Result<u32, FsBackendError> {
        if needed == 0 { return Ok(0); }
        let max=self.superblock.data_sectors;
        let mut run=0u32;
        let mut start=0u32;
        for block in 0..max {
            if !self.bitmap_bit(block)? {
                if run==0 { start=block; }
                run+=1;
                if run>=needed { return Ok(start); }
            } else {
                run=0;
            }
        }
        Err(FsBackendError::NoSpace)
    }

    fn mark_extent(&mut self, start: u32, count: u32) -> Result<(), FsBackendError> {
        if start.checked_add(count).unwrap_or(u32::MAX) > self.superblock.data_sectors {
            return Err(FsBackendError::CorruptMetadata);
        }
        for block in start..start+count { self.set_bitmap_bit(block,true)?; }
        Ok(())
    }

    fn free_extent(&mut self, start: u32, count: u32) -> Result<(), FsBackendError> {
        if start.checked_add(count).unwrap_or(u32::MAX) > self.superblock.data_sectors {
            return Err(FsBackendError::CorruptMetadata);
        }
        for block in start..start+count { self.set_bitmap_bit(block,false)?; }
        Ok(())
    }

    fn find_inode(&mut self, path: &str) -> Result<Option<(u32, DiskInode)>, FsBackendError> {
        for index in 0..MAX_INODES {
            let inode=self.read_inode(index)?;
            if inode.used && inode.path()? == path { return Ok(Some((index,inode))); }
        }
        Ok(None)
    }

    fn validate_path(path: &str) -> Result<(), FsBackendError> {
        if path.is_empty() || !path.starts_with('/') || path.len() > MAX_PATH || path.contains("//") {
            return Err(FsBackendError::InvalidPath);
        }
        Ok(())
    }

    pub fn create_file(&mut self, path: &str, data: &[u8]) -> Result<u32, FsBackendError> {
        Self::validate_path(path)?;
        if self.find_inode(path)?.is_some() { return Err(FsBackendError::InvalidPath); }
        let mut inode_index=None;
        for index in 0..MAX_INODES {
            if !self.read_inode(index)?.used { inode_index=Some(index); break; }
        }
        let index=inode_index.ok_or(FsBackendError::TooManyFiles)?;
        if data.len() > (self.superblock.data_sectors as usize).saturating_mul(SECTOR_SIZE) {
            return Err(FsBackendError::FileTooLarge);
        }
        let sectors=((data.len()+SECTOR_SIZE-1)/SECTOR_SIZE) as u32;
        let start=self.find_free_extent(sectors)?;
        let mut inode=DiskInode::empty();
        inode.used=true;
        inode.file_size=data.len() as u32;
        inode.start_sector=if sectors == 0 { 0 } else { start };
        inode.sector_count=sectors;
        inode.path_len=path.len() as u8;
        inode.path[..path.len()].copy_from_slice(path.as_bytes());
        let txn = FsTransaction {
            op: 1,
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
            if let Err(e)=self.device.write_sector(self.base_lba + self.superblock.data_start+start+block,&sector) {
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

    pub fn read_file(&mut self, path: &str, out: &mut [u8]) -> Result<usize, FsBackendError> {
        Self::validate_path(path)?;
        let (_index,inode)=self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        if inode.file_size == 0 { return Ok(0); }
        if inode.sector_count == 0 || inode.start_sector.checked_add(inode.sector_count).unwrap_or(u32::MAX) > self.superblock.data_sectors {
            return Err(FsBackendError::CorruptMetadata);
        }
        if out.len() < inode.file_size as usize { return Err(FsBackendError::FileTooLarge); }
        let mut copied=0usize;
        for block in 0..inode.sector_count {
            let mut sector=[0u8; SECTOR_SIZE];
            self.device.read_sector(self.base_lba + self.superblock.data_start+inode.start_sector+block,&mut sector)?;
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
        if data.len() > (self.superblock.data_sectors as usize).saturating_mul(SECTOR_SIZE) {
            return Err(FsBackendError::FileTooLarge);
        }
        let sectors=((data.len()+SECTOR_SIZE-1)/SECTOR_SIZE) as u32;
        let start=self.find_free_extent(sectors)?;
        let mut inode=old;
        inode.file_size=data.len() as u32;
        inode.start_sector=if sectors == 0 { 0 } else { start };
        inode.sector_count=sectors;
        let txn = FsTransaction {
            op: 2,
            inode_index: index,
            old_used: old.used, old_size: old.file_size, old_start: old.start_sector, old_count: old.sector_count,
            new_used: inode.used, new_size: inode.file_size, new_start: inode.start_sector, new_count: inode.sector_count,
        };
        if let Err(e)=self.write_transaction(&txn) {
            return Err(e);
        }
        if let Err(e)=self.mark_extent(start,sectors) {
            let _=self.clear_transaction();
            return Err(e);
        }
        for block in 0..sectors {
            let mut sector=[0u8; SECTOR_SIZE];
            let begin=block as usize*SECTOR_SIZE;
            let end=(begin+SECTOR_SIZE).min(data.len());
            if begin<end { sector[..end-begin].copy_from_slice(&data[begin..end]); }
            if let Err(e)=self.device.write_sector(self.base_lba + self.superblock.data_start+start+block,&sector) {
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

    pub fn delete_file(&mut self, path: &str) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        let (index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        let txn = FsTransaction {
            op: 3,
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
