//! Minimal on-disk InterOS filesystem backend.
//!
//! M20 introduces a real block-backed filesystem boundary above `BlockDevice`.
//! The format is intentionally small and deterministic: a superblock, allocation
//! bitmap, fixed-size inode table, and contiguous file data extents. VFS can use
//! this backend without knowing the physical ATA/SDHCI implementation.
//!
//! M22 will add transactional metadata publication and deterministic crash
//! recovery around these mutations.

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
        Ok(Self { device, superblock: sb, base_lba })
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
        DiskInode::decode(&raw)
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

    fn allocate_extent(&mut self, needed: u32) -> Result<u32, FsBackendError> {
        if needed == 0 { return Ok(0); }
        let max=self.superblock.data_sectors;
        let mut run=0u32;
        let mut start=0u32;
        for block in 0..max {
            if !self.bitmap_bit(block)? {
                if run==0 { start=block; }
                run+=1;
                if run>=needed {
                    for b in start..start+needed { self.set_bitmap_bit(b,true)?; }
                    return Ok(start);
                }
            } else {
                run=0;
            }
        }
        Err(FsBackendError::NoSpace)
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
        if !path.starts_with('/') || path.len() > MAX_PATH || path.contains("//") {
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
        let start=self.allocate_extent(sectors)?;
        for block in 0..sectors {
            let mut sector=[0u8; SECTOR_SIZE];
            let begin=block as usize*SECTOR_SIZE;
            let end=(begin+SECTOR_SIZE).min(data.len());
            if begin<end { sector[..end-begin].copy_from_slice(&data[begin..end]); }
            self.device.write_sector(self.base_lba + self.superblock.data_start+start+block,&sector)?;
        }
        let mut inode=DiskInode::empty();
        inode.used=true;
        inode.file_size=data.len() as u32;
        inode.start_sector=if sectors == 0 { 0 } else { start };
        inode.sector_count=sectors;
        inode.path_len=path.len() as u8;
        inode.path[..path.len()].copy_from_slice(path.as_bytes());
        if let Err(e)=self.write_inode(index,&inode) {
            let _=self.free_extent(start,sectors);
            return Err(e);
        }
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

    /// Replaces file contents. Metadata is published only after the new extent
    /// has been fully written; M22 will make bitmap/inode publication atomic.
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        let (index,old)=self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        if data.len() > (self.superblock.data_sectors as usize).saturating_mul(SECTOR_SIZE) {
            return Err(FsBackendError::FileTooLarge);
        }
        let sectors=((data.len()+SECTOR_SIZE-1)/SECTOR_SIZE) as u32;
        let start=self.allocate_extent(sectors)?;
        for block in 0..sectors {
            let mut sector=[0u8; SECTOR_SIZE];
            let begin=block as usize*SECTOR_SIZE;
            let end=(begin+SECTOR_SIZE).min(data.len());
            if begin<end { sector[..end-begin].copy_from_slice(&data[begin..end]); }
            self.device.write_sector(self.base_lba + self.superblock.data_start+start+block,&sector)?;
        }
        let mut inode=old;
        inode.file_size=data.len() as u32;
        inode.start_sector=if sectors == 0 { 0 } else { start };
        inode.sector_count=sectors;
        if let Err(e)=self.write_inode(index,&inode) {
            let _=self.free_extent(start,sectors);
            return Err(e);
        }
        let _=self.free_extent(old.start_sector,old.sector_count);
        Ok(())
    }

    pub fn delete_file(&mut self, path: &str) -> Result<(), FsBackendError> {
        Self::validate_path(path)?;
        let (index, inode) = self.find_inode(path)?.ok_or(FsBackendError::FileNotFound)?;
        if inode.sector_count != 0 {
            self.free_extent(inode.start_sector, inode.sector_count)?;
        }
        self.write_inode(index, &DiskInode::empty())?;
        Ok(())
    }
}
