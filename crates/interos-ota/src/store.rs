// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The durable, block-device-backed A/B slot store.
//!
//! WHAT IS ON THE MEDIA, in plain words:
//!
//! ```text
//!   LBA 2048   metadata copy A   (one sector: state + generation + digest)
//!   LBA 2049   metadata copy B   (one sector: state + generation + digest)
//!   LBA 2050   slot A header     (one sector: image length + image digest)
//!   LBA 2051.. slot A image      (up to 64 sectors = 32 KiB)
//!   LBA 2115   slot B header
//!   LBA 2116.. slot B image
//! ```
//!
//! THE INVARIANTS THIS FILE ENCODES:
//!
//! * I1 — A metadata record is believed only when its own SHA-256 verifies.
//! * I2 — The selected metadata is the verifying record with the highest
//!   generation; two copies at the same generation with different contents
//!   are refused, never guessed.
//! * I3 — The generation only increases: a commit writes `previous + 1`.
//! * I4 — A commit writes to the copy that does NOT hold the newest
//!   generation. A crash during that write leaves the previous generation
//!   intact, intact and selected. The device therefore always selects the
//!   previous generation after an interrupted commit.
//! * I5 — An image is written before the header that names it, and the header
//!   carries the image's SHA-256. A torn image is therefore detectable: the
//!   bytes are re-read and re-hashed.
//! * I6 — A successful commit is confirmed by reading the record back off the
//!   device. `Ok` means "the bytes came back", not "the write call returned".
//! * I7 — Nothing outside the layout is ever written. The store's writes are
//!   computed from the layout alone, so a user-data region on the same device
//!   cannot be reached by an OTA operation.

use alloc::vec;
use alloc::vec::Vec;

use crate::sha256::{constant_time_eq_32, sha256};
use crate::state::{
    select_latest, MetadataRecord, OtaError, OtaState, Slot, SECTOR_SIZE,
};

/// Slot header magic: "INTR_SLH" (Inter OS SLOT Header).
pub const SLOT_HEADER_MAGIC: [u8; 8] = *b"INTR_SLH";

/// Slot header format version.
pub const SLOT_HEADER_VERSION: u32 = 1;

/// Bytes of a slot header covered by the header digest.
const SLOT_HEADER_BODY_LEN: usize = 56;

/// Offset of the header digest inside a slot header.
const SLOT_HEADER_DIGEST_OFFSET: usize = SLOT_HEADER_BODY_LEN;

/// Errors the block layer reports.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockError {
    /// No device is bound.
    DeviceNotFound,
    /// The sector read failed.
    ReadError,
    /// The sector write failed.
    WriteError,
    /// The flush of volatile caches failed.
    FlushError,
    /// The sector address is outside the device.
    LbaOutOfRange,
}

/// The narrow block contract the OTA store needs.
///
/// WHY THIS TRAIT AND NOT THE KERNEL'S: the store must be testable on a host
/// with no MMIO, and the kernel's contract lives in an architecture tree that
/// is not a workspace member. The trait is four sector operations; the
/// architecture supplies the implementation (virtio-mmio on QEMU, an eMMC or
/// UFS controller on a phone) and nothing else.
pub trait BlockDevice {
    /// Reads one sector.
    fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockError>;

    /// Writes one sector.
    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockError>;

    /// Number of sectors the device holds.
    fn sector_count(&self) -> u32;

    /// Flushes volatile write caches to persistent media.
    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

/// The storage contract the OTA engine consumes.
pub trait OtaStore {
    /// The last committed state, or `Ok(None)` when nothing has been committed
    /// yet (a fresh device).
    fn load(&mut self) -> Result<Option<OtaState>, OtaError>;

    /// Commits a state. On `Ok` the state is on the device and has been read
    /// back. On `Err` the caller must not assume anything changed.
    fn save(&mut self, state: &OtaState) -> Result<(), OtaError>;

    /// Whether this backend holds slot images.
    fn holds_slot_content(&self) -> bool {
        false
    }

    /// Writes a slot image and publishes it with its slot header.
    fn write_slot(&mut self, slot: Slot, image: &[u8]) -> Result<(), OtaError>;

    /// Reads a slot image back, verifying it against the digest its header
    /// names. `Ok(None)` means the slot holds no image.
    fn read_slot(&mut self, slot: Slot) -> Result<Option<Vec<u8>>, OtaError>;
}

/// Where the OTA regions live on a device.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// First sector of metadata copy A.
    pub meta_a_lba: u32,
    /// First sector of metadata copy B.
    pub meta_b_lba: u32,
    /// First sector of slot A (its header sector).
    pub slot_a_lba: u32,
    /// First sector of slot B (its header sector).
    pub slot_b_lba: u32,
    /// Sectors available for each slot's image.
    pub slot_payload_sectors: u32,
}

impl Layout {
    /// A layout with the metadata copies, then the two slots, at `base`.
    pub const fn at(base: u32, slot_payload_sectors: u32) -> Layout {
        let slot_a_lba = base + 2;
        let slot_b_lba = slot_a_lba + 1 + slot_payload_sectors;
        Layout {
            meta_a_lba: base,
            meta_b_lba: base + 1,
            slot_a_lba,
            slot_b_lba,
            slot_payload_sectors,
        }
    }

    /// The layout used on the dedicated OTA device: it starts one mebibyte
    /// into the disk, so the first sectors of a disk that also carries a
    /// partition table are never in range of an OTA write.
    pub const fn default_ota_region() -> Layout {
        Layout::at(2048, 64)
    }

    /// The first sector of a slot's header.
    pub const fn slot_base(&self, slot: Slot) -> u32 {
        match slot {
            Slot::SlotA => self.slot_a_lba,
            Slot::SlotB => self.slot_b_lba,
        }
    }

    /// The first sector of a slot's image.
    pub const fn slot_payload_base(&self, slot: Slot) -> u32 {
        self.slot_base(slot) + 1
    }

    /// The first sector of metadata copy `index`.
    pub const fn meta_lba(&self, index: usize) -> u32 {
        if index == 0 {
            self.meta_a_lba
        } else {
            self.meta_b_lba
        }
    }

    /// Total sectors the layout occupies.
    pub const fn total_sectors(&self) -> u32 {
        self.slot_b_lba + 1 + self.slot_payload_sectors
    }

    /// Bytes an image may occupy in one slot.
    pub const fn slot_capacity_bytes(&self) -> usize {
        (self.slot_payload_sectors as usize) * SECTOR_SIZE
    }
}

/// A block-device-backed A/B OTA store.
///
/// WHY THE STORE OWNS THE GENERATION COUNTER: the generation is a property of
/// the media, not of the caller. Two engines on the same device must not be
/// able to publish the same generation, and the counter has to keep rising
/// across reboots. It therefore lives with the copies it describes.
pub struct BlockDeviceOtaStore<D> {
    device: D,
    layout: Layout,
    next_generation: u64,
    copies: [Option<MetadataRecord>; 2],
    boot_area_checked: bool,
}

impl<D: BlockDevice> BlockDeviceOtaStore<D> {
    /// Creates a store over `device` with the given layout.
    pub fn new(device: D, layout: Layout) -> BlockDeviceOtaStore<D> {
        BlockDeviceOtaStore {
            device,
            layout,
            next_generation: 0,
            copies: [None, None],
            boot_area_checked: false,
        }
    }

    /// The layout this store writes within.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The device this store writes to.
    pub fn device(&self) -> &D {
        &self.device
    }

    /// The device this store writes to, mutably.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    /// The generation the next commit will use.
    pub fn next_generation(&self) -> u64 {
        self.next_generation
    }

    /// Refuses to write to a device whose first sector carries a boot
    /// signature.
    ///
    /// WHY: the OTA region is not the only thing a disk can hold. A disk whose
    /// sector 0 ends in `55 AA` is a bootable disk, and writing an OTA region
    /// onto it can destroy the very thing that boots the device. The check
    /// runs once, before the first write, and the answer is remembered.
    fn check_boot_area(&mut self) -> Result<(), OtaError> {
        if self.boot_area_checked {
            return Ok(());
        }
        let mut sector = [0u8; SECTOR_SIZE];
        self.device
            .read_sector(0, &mut sector)
            .map_err(|_| OtaError::DeviceReadFailed)?;
        if sector[510] == 0x55 && sector[511] == 0xAA {
            return Err(OtaError::BootDeviceProtected);
        }
        self.boot_area_checked = true;
        Ok(())
    }

    /// Reads one sector, mapping the block error vocabulary.
    fn read_sector(&mut self, lba: u32) -> Result<[u8; SECTOR_SIZE], OtaError> {
        let mut buf = [0u8; SECTOR_SIZE];
        self.device
            .read_sector(lba, &mut buf)
            .map_err(|err| match err {
                BlockError::DeviceNotFound => OtaError::DeviceNotFound,
                BlockError::LbaOutOfRange => OtaError::DeviceOutOfRange,
                _ => OtaError::DeviceReadFailed,
            })?;
        Ok(buf)
    }

    /// Writes one sector, mapping the block error vocabulary.
    fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), OtaError> {
        self.device
            .write_sector(lba, buf)
            .map_err(|err| match err {
                BlockError::DeviceNotFound => OtaError::DeviceNotFound,
                BlockError::LbaOutOfRange => OtaError::DeviceOutOfRange,
                _ => OtaError::DeviceWriteFailed,
            })
    }

    /// Flushes the device, mapping the block error vocabulary.
    fn flush(&mut self) -> Result<(), OtaError> {
        self.device
            .flush()
            .map_err(|_| OtaError::DeviceFlushFailed)
    }

    /// Reads one metadata copy, returning `None` when it holds no verifying
    /// record.
    fn read_copy(&mut self, index: usize) -> Result<Option<MetadataRecord>, OtaError> {
        let bytes = self.read_sector(self.layout.meta_lba(index))?;
        match MetadataRecord::decode(&bytes) {
            Ok(record) => Ok(Some(record)),
            // A copy that does not verify is not a record. It does not make
            // the load fail: the other copy may still carry a good state.
            Err(_) => Ok(None),
        }
    }

    /// The copy a commit must write: the one that does not hold the newest
    /// generation. Writing the older copy is what makes an interrupted commit
    /// harmless (invariant I4).
    fn target_copy(&self) -> usize {
        let gen_a = self.copies[0].map(|r| r.generation).unwrap_or(0);
        let gen_b = self.copies[1].map(|r| r.generation).unwrap_or(0);
        if gen_a <= gen_b {
            0
        } else {
            1
        }
    }
}

impl<D: BlockDevice> OtaStore for BlockDeviceOtaStore<D> {
    fn load(&mut self) -> Result<Option<OtaState>, OtaError> {
        let total = self.device.sector_count();
        if total == 0 {
            return Err(OtaError::DeviceNotFound);
        }
        if total < self.layout.total_sectors() {
            return Err(OtaError::LayoutOutOfRange);
        }
        let copy_a = self.read_copy(0)?;
        let copy_b = self.read_copy(1)?;
        let selected = select_latest(copy_a, copy_b)?;
        self.copies = [copy_a, copy_b];
        self.next_generation = selected.map(|r| r.generation).unwrap_or(0);
        Ok(selected.map(|r| r.state))
    }

    fn save(&mut self, state: &OtaState) -> Result<(), OtaError> {
        let total = self.device.sector_count();
        if total == 0 {
            return Err(OtaError::DeviceNotFound);
        }
        if total < self.layout.total_sectors() {
            return Err(OtaError::LayoutOutOfRange);
        }
        self.check_boot_area()?;

        // WHY THE COPIES ARE READ BACK BEFORE THE TARGET IS CHOSEN: the store
        // may not have loaded since the device was last written — by another
        // writer, or by this store before a power cycle. A commit that picked
        // its target from a stale view would write generation one over the
        // copy that already holds the newest record, and the reader would
        // then select the *older* copy and lose the commit without ever
        // being told. The device, not the store's memory, decides where a
        // commit goes and what generation it carries.
        let copy_a = self.read_copy(0)?;
        let copy_b = self.read_copy(1)?;
        self.copies = [copy_a, copy_b];
        let newest = self.copies[0]
            .map(|record| record.generation)
            .unwrap_or(0)
            .max(self.copies[1].map(|record| record.generation).unwrap_or(0));

        let generation = newest.saturating_add(1);
        let record = MetadataRecord {
            generation,
            state: *state,
        };
        let bytes = record.encode();
        let target = self.target_copy();

        // The record is written whole: one sector, one write. A torn sector
        // fails its digest and is discarded by the reader, which then selects
        // the other copy — the previous generation.
        self.write_sector(self.layout.meta_lba(target), &bytes)?;
        self.flush()?;

        // Read-back verification: `Ok` means the bytes came back off the
        // device and describe exactly what was committed (invariant I6).
        let readback = self.read_sector(self.layout.meta_lba(target))?;
        let decoded = MetadataRecord::decode(&readback).map_err(|_| OtaError::PersistenceFailed)?;
        if decoded.generation != generation || decoded.state != *state {
            return Err(OtaError::PersistenceFailed);
        }

        self.copies[target] = Some(record);
        self.next_generation = generation;
        Ok(())
    }

    fn holds_slot_content(&self) -> bool {
        true
    }

    fn write_slot(&mut self, slot: Slot, image: &[u8]) -> Result<(), OtaError> {
        let capacity = self.layout.slot_capacity_bytes();
        if image.len() > capacity {
            return Err(OtaError::SlotContentTooLarge);
        }
        self.check_boot_area()?;

        // The image first, then the header that names it. The header is the
        // only thing that publishes the slot, so a crash between the two
        // leaves an unpublished image that no reader will believe.
        let payload_base = self.layout.slot_payload_base(slot);
        let mut sector = [0u8; SECTOR_SIZE];
        let chunks: Vec<&[u8]> = image.chunks(SECTOR_SIZE).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            sector.fill(0);
            sector[..chunk.len()].copy_from_slice(chunk);
            self.write_sector(payload_base + index as u32, &sector)?;
        }
        self.flush()?;

        let header = encode_slot_header(image.len() as u32, sha256(image));
        self.write_sector(self.layout.slot_base(slot), &header)?;
        self.flush()?;
        Ok(())
    }

    fn read_slot(&mut self, slot: Slot) -> Result<Option<Vec<u8>>, OtaError> {
        let header = self.read_sector(self.layout.slot_base(slot))?;
        if header[0..8] != SLOT_HEADER_MAGIC {
            // No header: the slot holds no published image.
            return Ok(None);
        }
        let (len, digest) = decode_slot_header(&header)?;
        let capacity = self.layout.slot_capacity_bytes();
        if len as usize > capacity {
            return Err(OtaError::SlotContentTooLarge);
        }
        let mut out = vec![0u8; len as usize];
        let payload_base = self.layout.slot_payload_base(slot);
        let mut written = 0usize;
        while written < out.len() {
            let index = written / SECTOR_SIZE;
            let sector = self.read_sector(payload_base + index as u32)?;
            let take = (out.len() - written).min(SECTOR_SIZE);
            out[written..written + take].copy_from_slice(&sector[..take]);
            written += take;
        }
        // The bytes that came back are hashed again here. This is the check
        // that makes a torn image detectable (invariant I5).
        if !constant_time_eq_32(&sha256(&out), &digest) {
            return Err(OtaError::SlotDigestMismatch);
        }
        Ok(Some(out))
    }
}

/// Encodes a slot header naming an image.
fn encode_slot_header(len: u32, digest: [u8; 32]) -> [u8; SECTOR_SIZE] {
    let mut out = [0u8; SECTOR_SIZE];
    out[0..8].copy_from_slice(&SLOT_HEADER_MAGIC);
    out[8..12].copy_from_slice(&SLOT_HEADER_VERSION.to_le_bytes());
    out[12..20].copy_from_slice(&0u64.to_le_bytes());
    out[20..24].copy_from_slice(&len.to_le_bytes());
    out[24..56].copy_from_slice(&digest);
    let header_digest = sha256(&out[..SLOT_HEADER_BODY_LEN]);
    out[SLOT_HEADER_DIGEST_OFFSET..SLOT_HEADER_DIGEST_OFFSET + 32].copy_from_slice(&header_digest);
    out
}

/// Decodes a slot header, verifying the header's own digest.
fn decode_slot_header(header: &[u8; SECTOR_SIZE]) -> Result<(u32, [u8; 32]), OtaError> {
    let version = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    if version != SLOT_HEADER_VERSION {
        return Err(OtaError::SlotHeaderCorrupt);
    }
    let mut expected = [0u8; 32];
    expected.copy_from_slice(&header[SLOT_HEADER_DIGEST_OFFSET..SLOT_HEADER_DIGEST_OFFSET + 32]);
    if !constant_time_eq_32(&sha256(&header[..SLOT_HEADER_BODY_LEN]), &expected) {
        return Err(OtaError::SlotHeaderCorrupt);
    }
    let len = u32::from_le_bytes([header[20], header[21], header[22], header[23]]);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&header[24..56]);
    Ok((len, digest))
}
