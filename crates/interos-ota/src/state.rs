// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The durable OTA state and its on-media encoding.
//!
//! WHAT IS ON THE MEDIA, in plain words: two slots, and two copies of one
//! small metadata record. The metadata record names the slot the device should
//! boot, and for each slot it names the version, the slot's status, the boot
//! attempts still left, the length of the slot's image and the SHA-256 of that
//! image. Each metadata copy carries a generation counter and a SHA-256 over
//! its own bytes. Each slot carries a header sector that names the length and
//! the SHA-256 of the image stored in that slot.
//!
//! THE INVARIANTS THIS FILE ENCODES:
//!
//! * I1 — A metadata record is only believed when its own digest verifies. A
//!   torn or garbled record is not "partly true"; it is not a record at all.
//! * I2 — The selected metadata is the verifying record with the highest
//!   generation. Two copies with the same generation and different contents
//!   are ambiguous and are refused, never guessed.
//! * I3 — The generation counter only ever increases. A commit writes the
//!   generation `previous + 1`, so a stale copy can never displace a newer
//!   one.
//! * I4 — A metadata record may only name an image digest that the slot's own
//!   header also names. Promotion therefore re-reads the bytes and compares
//!   the digest the record expects against the digest the bytes actually
//!   produce; a record alone never marks a slot valid.
//! * I5 — A slot whose status is `Unverified` or `Corrupted` is never selected
//!   for boot.

use crate::sha256::{constant_time_eq_32, sha256};

/// Hardware sector size assumed by the block layer.
pub const SECTOR_SIZE: usize = 512;

/// Metadata record magic: "INTR_OMD" (Inter OS OTA MetaData).
pub const META_MAGIC: [u8; 8] = *b"INTR_OMD";

/// Metadata record format version.
pub const META_VERSION: u32 = 1;

/// Size of one metadata record: exactly one sector.
pub const META_RECORD_SIZE: usize = SECTOR_SIZE;

/// Number of bytes of a metadata record that the record digest covers.
pub const META_BODY_LEN: usize = 112;

/// Offset of the record digest inside a metadata record.
pub const META_DIGEST_OFFSET: usize = META_BODY_LEN;

/// Errors the OTA subsystem reports.
///
/// WHY THE VARIANTS ARE DISTINCT: a caller has to be able to tell "the device
/// said no" (an I/O failure) from "the device said something I cannot trust"
/// (a corrupt record) from "there is nothing to boot" (no bootable slot).
/// Folding them together would let a failed write look like a fresh device.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OtaError {
    /// No block device is bound to the store.
    DeviceNotFound,
    /// A sector read failed.
    DeviceReadFailed,
    /// A sector write failed.
    DeviceWriteFailed,
    /// The device's flush of volatile caches failed.
    DeviceFlushFailed,
    /// A sector address is outside the device.
    DeviceOutOfRange,
    /// The OTA region does not fit on the device.
    LayoutOutOfRange,
    /// A metadata record was present but its digest did not verify.
    MetadataCorrupt,
    /// Two metadata copies claim the same generation with different contents.
    MetadataAmbiguous,
    /// A slot header was present but its digest did not verify.
    SlotHeaderCorrupt,
    /// The slot's stored bytes do not hash to the digest the slot header names.
    SlotDigestMismatch,
    /// The slot holds no image at all.
    SlotContentMissing,
    /// The image does not fit in the slot.
    SlotContentTooLarge,
    /// The backend cannot store slot images.
    SlotContentUnsupported,
    /// The metadata commit did not reach the device.
    PersistenceFailed,
    /// The candidate version is not newer than every version the device has
    /// already accepted.
    AntiRollback,
    /// The device's first sector carries a boot signature, so the OTA region
    /// would land on a disk that still has to boot something.
    BootDeviceProtected,
    /// No slot in the state may be booted.
    NoBootableSlot,
    /// The requested slot is not in a state that allows the operation.
    SlotNotBootable,
}

/// One of the two update slots.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    SlotA,
    SlotB,
}

impl Slot {
    /// The other slot.
    pub fn alternate(self) -> Slot {
        match self {
            Slot::SlotA => Slot::SlotB,
            Slot::SlotB => Slot::SlotA,
        }
    }

    /// Index of this slot in the metadata record (0 = A, 1 = B).
    pub fn index(self) -> usize {
        match self {
            Slot::SlotA => 0,
            Slot::SlotB => 1,
        }
    }

    /// The slot with the given index, if the index is one of the two slots.
    pub fn from_index(index: usize) -> Option<Slot> {
        match index {
            0 => Some(Slot::SlotA),
            1 => Some(Slot::SlotB),
            _ => None,
        }
    }
}

/// The state of a slot, as recorded in the metadata.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotStatus {
    /// The image was verified on the device and the boot it came from
    /// succeeded. Bootable.
    Valid,
    /// The image was verified on the device and the slot is on a trial boot.
    /// Bootable, and the boot attempts are counted down.
    Testing,
    /// An image is present but has NOT been verified against the digest the
    /// metadata expects. Never bootable: this is the state an interrupted
    /// update leaves behind.
    Unverified,
    /// The image failed verification. Never bootable.
    Corrupted,
}

impl SlotStatus {
    /// Whether a slot in this state may be selected for boot.
    ///
    /// WHY `Valid` AND `Testing` AND NOTHING ELSE: `Unverified` is what an
    /// interrupted update leaves on the media, and `Corrupted` is what a failed
    /// verification leaves. Neither may ever be booted.
    pub fn is_bootable(self) -> bool {
        matches!(self, SlotStatus::Valid | SlotStatus::Testing)
    }

    /// The byte this status is stored as.
    pub fn as_u8(self) -> u8 {
        match self {
            SlotStatus::Valid => 0,
            SlotStatus::Testing => 1,
            SlotStatus::Unverified => 2,
            SlotStatus::Corrupted => 3,
        }
    }

    /// The status stored as the given byte, if it is one this build knows.
    pub fn from_u8(value: u8) -> Option<SlotStatus> {
        match value {
            0 => Some(SlotStatus::Valid),
            1 => Some(SlotStatus::Testing),
            2 => Some(SlotStatus::Unverified),
            3 => Some(SlotStatus::Corrupted),
            _ => None,
        }
    }
}

/// What the metadata records about one slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlotMetadata {
    /// Monotonic version of the image in this slot.
    pub version: u32,
    /// State of the slot.
    pub status: SlotStatus,
    /// Boot attempts still available before the slot is abandoned.
    pub boot_tries_left: u8,
    /// Length in bytes of the image stored in this slot.
    pub content_len: u32,
    /// SHA-256 of the image stored in this slot.
    pub content_digest: [u8; 32],
}

impl SlotMetadata {
    /// The metadata of a slot that holds no image.
    pub const EMPTY: SlotMetadata = SlotMetadata {
        version: 0,
        status: SlotStatus::Corrupted,
        boot_tries_left: 0,
        content_len: 0,
        content_digest: [0u8; 32],
    };
}

/// The complete durable OTA state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OtaState {
    /// The slot the device should boot.
    pub active_slot: Slot,
    /// Metadata of slot A.
    pub slot_a: SlotMetadata,
    /// Metadata of slot B.
    pub slot_b: SlotMetadata,
}

impl OtaState {
    /// The state of a device that has never been updated: slot A holds the
    /// running image, slot B holds nothing.
    pub fn initial(current_version: u32) -> OtaState {
        OtaState {
            active_slot: Slot::SlotA,
            slot_a: SlotMetadata {
                version: current_version,
                status: SlotStatus::Valid,
                boot_tries_left: 3,
                content_len: 0,
                content_digest: [0u8; 32],
            },
            slot_b: SlotMetadata::EMPTY,
        }
    }

    /// The metadata of the given slot.
    pub fn slot(&self, slot: Slot) -> &SlotMetadata {
        match slot {
            Slot::SlotA => &self.slot_a,
            Slot::SlotB => &self.slot_b,
        }
    }

    /// The metadata of the given slot, for mutation.
    pub fn slot_mut(&mut self, slot: Slot) -> &mut SlotMetadata {
        match slot {
            Slot::SlotA => &mut self.slot_a,
            Slot::SlotB => &mut self.slot_b,
        }
    }

    /// The metadata of the slot the device boots.
    pub fn active(&self) -> &SlotMetadata {
        self.slot(self.active_slot)
    }

    /// The highest version recorded in either slot.
    ///
    /// WHY THE MAXIMUM AND NOT THE ACTIVE SLOT'S VERSION: anti-rollback has to
    /// compare against the newest version the device has ever accepted. If a
    /// rollback moved the active pointer back to an older slot, comparing only
    /// against that slot would let the attacker replay the version the device
    /// just abandoned.
    pub fn highest_version(&self) -> u32 {
        self.slot_a.version.max(self.slot_b.version)
    }
}

/// One metadata record: a generation number and the state it describes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MetadataRecord {
    /// Monotonically increasing commit counter.
    pub generation: u64,
    /// The state this record publishes.
    pub state: OtaState,
}

impl MetadataRecord {
    /// Encodes this record into exactly one sector.
    ///
    /// WHY THE DIGEST IS OVER A FIXED PREFIX: the body is a fixed 112-byte
    /// layout with every field at a fixed offset, so the digest covers every
    /// field without any ambiguity about what was hashed. The remaining bytes
    /// of the sector are zero padding and are not covered — they carry no
    /// information.
    pub fn encode(&self) -> [u8; META_RECORD_SIZE] {
        let mut out = [0u8; META_RECORD_SIZE];
        out[0..8].copy_from_slice(&META_MAGIC);
        out[8..12].copy_from_slice(&META_VERSION.to_le_bytes());
        out[12..20].copy_from_slice(&self.generation.to_le_bytes());
        out[20] = self.state.active_slot.index() as u8;
        encode_slot(&mut out, 24, &self.state.slot_a);
        encode_slot(&mut out, 68, &self.state.slot_b);
        let digest = sha256(&out[..META_BODY_LEN]);
        out[META_DIGEST_OFFSET..META_DIGEST_OFFSET + 32].copy_from_slice(&digest);
        out
    }

    /// Decodes a record from a sector, verifying the record digest.
    ///
    /// A record whose digest does not verify is refused (`MetadataCorrupt`):
    /// it is not a record that can be partly believed, and a torn write must
    /// never be able to produce a state that looks like a commit.
    pub fn decode(bytes: &[u8]) -> Result<MetadataRecord, OtaError> {
        if bytes.len() < META_RECORD_SIZE {
            return Err(OtaError::MetadataCorrupt);
        }
        if bytes[0..8] != META_MAGIC {
            return Err(OtaError::MetadataCorrupt);
        }
        let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if version != META_VERSION {
            return Err(OtaError::MetadataCorrupt);
        }
        let mut expected = [0u8; 32];
        expected.copy_from_slice(&bytes[META_DIGEST_OFFSET..META_DIGEST_OFFSET + 32]);
        let actual = sha256(&bytes[..META_BODY_LEN]);
        if !constant_time_eq_32(&actual, &expected) {
            return Err(OtaError::MetadataCorrupt);
        }
        let generation = u64::from_le_bytes([
            bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17], bytes[18], bytes[19],
        ]);
        let active_slot = Slot::from_index(bytes[20] as usize).ok_or(OtaError::MetadataCorrupt)?;
        let slot_a = decode_slot(bytes, 24)?;
        let slot_b = decode_slot(bytes, 68)?;
        Ok(MetadataRecord {
            generation,
            state: OtaState {
                active_slot,
                slot_a,
                slot_b,
            },
        })
    }
}

/// Writes one slot's metadata at the given offset.
fn encode_slot(out: &mut [u8; META_RECORD_SIZE], offset: usize, meta: &SlotMetadata) {
    out[offset..offset + 4].copy_from_slice(&meta.version.to_le_bytes());
    out[offset + 4] = meta.status.as_u8();
    out[offset + 5] = meta.boot_tries_left;
    out[offset + 8..offset + 12].copy_from_slice(&meta.content_len.to_le_bytes());
    out[offset + 12..offset + 44].copy_from_slice(&meta.content_digest);
}

/// Reads one slot's metadata from the given offset.
fn decode_slot(bytes: &[u8], offset: usize) -> Result<SlotMetadata, OtaError> {
    let version = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]);
    let status = SlotStatus::from_u8(bytes[offset + 4]).ok_or(OtaError::MetadataCorrupt)?;
    let boot_tries_left = bytes[offset + 5];
    let content_len = u32::from_le_bytes([
        bytes[offset + 8],
        bytes[offset + 9],
        bytes[offset + 10],
        bytes[offset + 11],
    ]);
    let mut content_digest = [0u8; 32];
    content_digest.copy_from_slice(&bytes[offset + 12..offset + 44]);
    Ok(SlotMetadata {
        version,
        status,
        boot_tries_left,
        content_len,
        content_digest,
    })
}

/// Chooses between the two metadata copies.
///
/// WHY THE ORDER MATTERS: the copy with the highest generation is the one
/// that was committed last, so it wins. A copy that fails its digest check is
/// not a record at all and never competes. When both copies carry the same
/// generation but describe different states, the device is in a state this
/// code cannot explain, and guessing between them is exactly the kind of
/// choice that boots the wrong image — so it is refused.
pub fn select_latest(
    copy_a: Option<MetadataRecord>,
    copy_b: Option<MetadataRecord>,
) -> Result<Option<MetadataRecord>, OtaError> {
    match (copy_a, copy_b) {
        (None, None) => Ok(None),
        (Some(a), None) => Ok(Some(a)),
        (None, Some(b)) => Ok(Some(b)),
        (Some(a), Some(b)) => {
            if a.generation > b.generation {
                Ok(Some(a))
            } else if b.generation > a.generation {
                Ok(Some(b))
            } else if a.state == b.state {
                Ok(Some(a))
            } else {
                Err(OtaError::MetadataAmbiguous)
            }
        }
    }
}
