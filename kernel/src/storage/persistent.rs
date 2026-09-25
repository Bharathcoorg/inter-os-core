//! Crash-consistent persistent metadata journal for InterOS.
//!
//! Sixteen redundant slots are partitioned into eight two-slot record kinds so
//! package-registry, revocation, trust, VFS, and package-payload state cannot overwrite one another. Payload sectors are written
//! before the generation-bearing header; boot selects the newest digest-valid
//! record for the requested kind.

use crate::crypto::Sha256;
use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};

pub const JOURNAL_MAGIC: [u8; 4] = *b"IOSJ";
pub const JOURNAL_VERSION: u32 = 1;
const SLOT_SECTORS: u32 = 128;
const SLOT_COUNT: u32 = 16;
const HEADER_SIZE: usize = 64;
const PAYLOAD_CAPACITY: usize = (SLOT_SECTORS as usize * SECTOR_SIZE) - HEADER_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentError {
    Unavailable,
    Corrupt,
    TooLarge,
    Io,
}

fn slot_base(total: u32, slot: u32) -> Option<u32> {
    let reserved = SLOT_SECTORS.checked_mul(SLOT_COUNT)?;
    if total < reserved + 1 || slot >= SLOT_COUNT {
        return None;
    }
    Some(total - reserved + slot * SLOT_SECTORS)
}

fn read_slot(
    drive: &mut impl BlockDevice,
    base: u32,
    payload: &mut [u8],
    expected_kind: u8,
) -> Result<Option<(u64, usize)>, PersistentError> {
    let mut header = [0u8; SECTOR_SIZE];
    drive.read_sector(base, &mut header).map_err(|_| PersistentError::Io)?;

    if header[0..4] != JOURNAL_MAGIC
        || u32::from_le_bytes([header[4], header[5], header[6], header[7]]) != JOURNAL_VERSION
        || header[20] != expected_kind
    {
        return Ok(None);
    }

    let generation = u64::from_le_bytes([
        header[8], header[9], header[10], header[11],
        header[12], header[13], header[14], header[15],
    ]);
    let len = u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
    if len == 0 || len > PAYLOAD_CAPACITY || len > payload.len() {
        return Ok(None);
    }

    let mut digest = [0u8; 32];
    digest.copy_from_slice(&header[24..56]);

    let mut offset = 0usize;
    for sector in 1..SLOT_SECTORS {
        let mut buf = [0u8; SECTOR_SIZE];
        drive.read_sector(base + sector, &mut buf).map_err(|_| PersistentError::Io)?;
        let take = (len - offset).min(SECTOR_SIZE);
        if take > 0 {
            payload[offset..offset + take].copy_from_slice(&buf[..take]);
            offset += take;
        }
        if offset >= len {
            break;
        }
    }

    if offset != len || Sha256::digest(&payload[..len]) != digest {
        return Ok(None);
    }
    Ok(Some((generation, len)))
}

pub fn load_latest_kind(
    out: &mut [u8],
    kind: u8,
) -> Result<Option<(u64, usize)>, PersistentError> {
    let mut guard = ATA_PRIMARY.lock();
    let drive = guard.as_mut().ok_or(PersistentError::Unavailable)?;
    let total = drive.sector_count();

    let mut best: Option<(u64, usize)> = None;
    for slot in 0..SLOT_COUNT {
        let Some(base) = slot_base(total, slot) else { continue; };
        let mut candidate = [0u8; PAYLOAD_CAPACITY];
        if let Some((generation, len)) = read_slot(drive, base, &mut candidate, kind)? {
            if best.map(|(g, _)| generation > g).unwrap_or(true) {
                if len <= out.len() {
                    out[..len].copy_from_slice(&candidate[..len]);
                    best = Some((generation, len));
                }
            }
        }
    }
    Ok(best)
}

pub fn commit_kind(payload: &[u8], kind: u8) -> Result<u64, PersistentError> {
    if payload.is_empty() || payload.len() > PAYLOAD_CAPACITY {
        return Err(PersistentError::TooLarge);
    }

    let mut guard = ATA_PRIMARY.lock();
    let drive = guard.as_mut().ok_or(PersistentError::Unavailable)?;
    let total = drive.sector_count();

    let mut generations = [0u64; SLOT_COUNT as usize];
    for slot in 0..SLOT_COUNT {
        let base = slot_base(total, slot).ok_or(PersistentError::Unavailable)?;
        let mut buf = [0u8; PAYLOAD_CAPACITY];
        if let Some((generation, _)) = read_slot(drive, base, &mut buf, kind)? {
            generations[slot as usize] = generation;
        }
    }

    // Keep two slots per record kind. Eight kinds are available, each with an
    // independent alternating pair so a partially written record never replaces
    // the last known-good generation.
    let pair_start = match kind {
        1 => 0usize, 2 => 2usize, 3 => 4usize, 4 => 6usize,
        5 => 8usize, 6 => 10usize, 7 => 12usize, 8 => 14usize,
        _ => return Err(PersistentError::Unavailable),
    };
    let a = pair_start;
    let b = pair_start + 1;
    let target = if generations[a] <= generations[b] { a } else { b };
    let next_generation = generations[a].max(generations[b]).saturating_add(1);
    let base = slot_base(total, target as u32).ok_or(PersistentError::Unavailable)?;

    for sector in 1..SLOT_SECTORS {
        let mut buf = [0u8; SECTOR_SIZE];
        let start = ((sector - 1) as usize) * SECTOR_SIZE;
        if start < payload.len() {
            let take = (payload.len() - start).min(SECTOR_SIZE);
            buf[..take].copy_from_slice(&payload[start..start + take]);
        }
        drive.write_sector(base + sector, &buf).map_err(|_| PersistentError::Io)?;
    }

    let digest = Sha256::digest(payload);
    let mut header = [0u8; SECTOR_SIZE];
    header[0..4].copy_from_slice(&JOURNAL_MAGIC);
    header[4..8].copy_from_slice(&JOURNAL_VERSION.to_le_bytes());
    header[8..16].copy_from_slice(&next_generation.to_le_bytes());
    header[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    header[20] = kind;
    header[24..56].copy_from_slice(&digest);
    header[56..60].copy_from_slice(&0x494F_534Au32.to_le_bytes());
    drive.write_sector(base, &header).map_err(|_| PersistentError::Io)?;

    Ok(next_generation)
}

pub fn load_latest(out: &mut [u8]) -> Result<Option<(u64, usize)>, PersistentError> {
    load_latest_kind(out, 0)
}

pub fn commit(payload: &[u8]) -> Result<u64, PersistentError> {
    commit_kind(payload, 0)
}
