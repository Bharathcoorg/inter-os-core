//! Small crash-consistent persistent journal for InterOS metadata.
//!
//! The journal deliberately sits below the VFS/package layers. It uses the real
//! ATA block device when present and keeps two fixed-size slots near the end of
//! the disk. A new record is written completely before its generation-bearing
//! header is committed, so boot can select the newest valid record after a
//! power loss.

use crate::crypto::Sha256;
use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};

pub const JOURNAL_MAGIC: [u8; 4] = *b"IOSJ";
pub const JOURNAL_VERSION: u32 = 1;
const SLOT_SECTORS: u32 = 4;
const SLOT_COUNT: u32 = 2;
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
) -> Result<Option<u64>, PersistentError> {
    let mut header = [0u8; SECTOR_SIZE];
    drive.read_sector(base, &mut header).map_err(|_| PersistentError::Io)?;

    if header[0..4] != JOURNAL_MAGIC {
        return Ok(None);
    }
    if u32::from_le_bytes([header[4], header[5], header[6], header[7]]) != JOURNAL_VERSION {
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

    if Sha256::digest(&payload[..len]) != digest {
        return Ok(None);
    }
    Ok(Some(generation))
}

pub fn load_latest(out: &mut [u8]) -> Result<Option<(u64, usize)>, PersistentError> {
    let mut guard = ATA_PRIMARY.lock();
    let drive = guard.as_mut().ok_or(PersistentError::Unavailable)?;
    let total = drive.sector_count();

    let mut best: Option<(u64, usize)> = None;
    let mut scratch = [0u8; PAYLOAD_CAPACITY];
    for slot in 0..SLOT_COUNT {
        let base = match slot_base(total, slot) {
            Some(v) => v,
            None => continue,
        };
        let mut candidate = [0u8; PAYLOAD_CAPACITY];
        if let Some(generation) = read_slot(drive, base, &mut candidate)? {
            if best.map(|(g, _)| generation > g).unwrap_or(true) {
                let len = candidate.iter().rposition(|b| *b != 0).map(|i| i + 1).unwrap_or(0);
                if len <= out.len() {
                    out[..len].copy_from_slice(&candidate[..len]);
                    best = Some((generation, len));
                }
            }
        }
    }
    scratch.fill(0);
    Ok(best)
}

pub fn commit(payload: &[u8]) -> Result<u64, PersistentError> {
    if payload.is_empty() || payload.len() > PAYLOAD_CAPACITY {
        return Err(PersistentError::TooLarge);
    }

    let mut guard = ATA_PRIMARY.lock();
    let drive = guard.as_mut().ok_or(PersistentError::Unavailable)?;
    let total = drive.sector_count();

    let mut generations = [0u64; SLOT_COUNT as usize];
    for slot in 0..SLOT_COUNT {
        let base = match slot_base(total, slot) {
            Some(v) => v,
            None => return Err(PersistentError::Unavailable),
        };
        let mut buf = [0u8; PAYLOAD_CAPACITY];
        if let Some(generation) = read_slot(drive, base, &mut buf)? {
            generations[slot as usize] = generation;
        }
    }

    let target = if generations[0] <= generations[1] { 0 } else { 1 };
    let next_generation = generations[0].max(generations[1]).saturating_add(1);
    let base = slot_base(total, target).ok_or(PersistentError::Unavailable)?;

    // Write payload sectors first. An incomplete write has no valid header.
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
    header[24..56].copy_from_slice(&digest);
    header[56..60].copy_from_slice(&0x494F_534A_u32.to_le_bytes());
    drive.write_sector(base, &header).map_err(|_| PersistentError::Io)?;

    Ok(next_generation)
}
