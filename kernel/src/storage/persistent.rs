//! Crash-consistent persistent metadata journal for InterOS.
//!
//! Redundant slots are partitioned into two-slot record kinds so
//! package-registry, revocation, trust, VFS, and package-payload state cannot
//! overwrite one another. Payload sectors are written before the
//! generation-bearing header; boot selects the newest digest-valid record for
//! the requested kind.
//!
//! The region is split in two so that a kind added after the primary layout was
//! frozen does not move any existing record:
//!
//! * the *primary* region is the final `SLOT_COUNT * SLOT_SECTORS` sectors and
//!   holds one alternating pair per kind `1..=8`;
//! * the *extension* region is the `EXTENSION_SLOT_COUNT * SLOT_SECTORS`
//!   sectors immediately below it, and holds the pairs for kinds added later.
//!
//! Every primary slot keeps the sector address it has always had. The extension
//! region is carved from the unallocated gap between the InterFS region (which
//! the bitmap caps at 32809 sectors, ending at LBA 36905) and the journal, which
//! on the 64 MiB reference image begins at sector 129024 — so claiming it
//! overlaps nothing that is allocated today.

use alloc::vec::Vec;
use spin::Mutex;
use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};
use crate::storage::boot_guard::{self, SectorRange, WriteVerdict};

pub const JOURNAL_MAGIC: [u8; 4] = *b"IOSJ";
pub const JOURNAL_VERSION: u32 = 1;
pub const SLOT_SECTORS: u32 = 128;
/// Slots in the primary region: one alternating pair for each of kinds 1..=8.
/// This literal is pinned by tests/test_storage_contract.py; see that test.
const SLOT_COUNT: u32 = 16;
/// Slots below the primary region, for kinds whose number is above 8.
const EXTENSION_SLOT_COUNT: u32 = 4;
/// Every slot the journal scans, in address order.
///
/// WHY this is `pub`: `storage::filesystem` derives its tail reservation from
/// this count, so the two cannot drift apart. When kind 9 gained an extension
/// pair, the reservation here had been left at `16 * 128`, which no longer
/// covered the journal -- harmless only because the InterFS size cap bound
/// first, but a latent way for the filesystem to format over a live journal.
pub const TOTAL_SLOT_COUNT: u32 = SLOT_COUNT + EXTENSION_SLOT_COUNT;
pub const JOURNAL_MIN_KIND: u8 = 1;
pub const JOURNAL_MAX_KIND: u8 = 10;
/// Payload bytes one slot can hold.
///
/// WHY this is `SLOT_SECTORS - 1` sectors and not `SLOT_SECTORS * 512 - 64`:
/// the header occupies sector 0 of the slot, and both the write loop and the
/// read loop only ever touch `1..SLOT_SECTORS`, so the slot physically holds
/// 127 payload sectors. The old expression was 448 bytes larger than that. A
/// record between the two sizes was accepted by `commit_kind`, published with a
/// valid tag, and then discarded by the read path's tag check — reported to the
/// caller as "no record", which is committed state silently vanishing. The
/// advertised capacity has to be the real one, so an oversized commit is
/// refused instead of lost.
const PAYLOAD_CAPACITY: usize = ((SLOT_SECTORS - 1) as usize) * SECTOR_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistentError {
    Unavailable,
    Corrupt,
    TooLarge,
    Io,
    /// The journal's tail region is inside the bootable region of the device the
    /// kernel booted from, so the commit was declined.
    ///
    /// WHY this is not `Unavailable`: `Unavailable` is what the recovery policy
    /// already treats as "refuse and let the caller re-seed", so folding this in
    /// would let a boot device that cannot be written masquerade as a device with
    /// nothing committed on it. The caller has to be able to tell "no record yet"
    /// from "this device would have been destroyed".
    BootDeviceProtected,
}

/// Sectors occupied by the whole journal: the extension region plus the primary
/// region, contiguous at the tail of the device.
fn journal_reserved() -> Option<u32> {
    SLOT_SECTORS.checked_mul(TOTAL_SLOT_COUNT)
}

/// Sector address of one journal slot.
///
/// WHY the primary region is anchored at the *top* of the reserved area: it has
/// to stay exactly where it has always been. Anchoring slot 0 at
/// `total - primary_reserved` means every existing record keeps its address when
/// a new kind is given a slot, so no journal migration is ever needed. The
/// extension region therefore occupies the sectors *below* the primary base,
/// and adding kinds never shifts the primary region.
fn slot_base(total: u32, slot: u32) -> Option<u32> {
    let reserved = journal_reserved()?;
    if total < reserved + 1 || slot >= TOTAL_SLOT_COUNT {
        return None;
    }
    let region_base = total - reserved;
    if slot < SLOT_COUNT {
        // Identical to the historical primary-region addresses.
        Some(region_base + EXTENSION_SLOT_COUNT * SLOT_SECTORS + slot * SLOT_SECTORS)
    } else {
        // Kind 9 originally occupied the first extension pair at
        // total - 18*128. Keep that pair byte-for-byte stable while allocating
        // the new kind 10 in the newly reserved pair immediately below it.
        match slot {
            16 | 17 => Some(region_base + (slot - SLOT_COUNT) * SLOT_SECTORS),
            18 | 19 => Some(region_base + 2 * SLOT_SECTORS + (slot - 18) * SLOT_SECTORS),
            _ => None,
        }
    }
}

/// The whole journal footprint, both the primary region and the extension
/// below it, as one half-open sector range.
fn journal_range(total: u32) -> Option<SectorRange> {
    SectorRange::new(total.checked_sub(journal_reserved()?)?, journal_reserved()?)
}

/// Refuses a journal commit that would land inside the bootable region of the
/// device the kernel is executing from.
///
/// WHY the whole journal is checked rather than just the one slot being
/// written: the boot area is not a sparse thing, and a commit to any slot of the
/// journal on a device whose boot area reaches the disk tail is writing over the
/// image that is currently running. Refusing the region keeps the answer
/// independent of which slot the alternating pair happened to pick.
///
/// WHY the guard is inside `commit_to` and not at the callers: every commit
/// path funnels through it, so there is exactly one place where a destructive
/// write can be declined, and the load path stays usable for inspection.
fn guard_commit(drive: &mut impl BlockDevice, total: u32) -> Result<(), PersistentError> {
    let Some(range) = journal_range(total) else {
        return Err(PersistentError::Unavailable);
    };
    match boot_guard::evaluate_device_writes(drive, range) {
        WriteVerdict::Permitted => Ok(()),
        WriteVerdict::Refused(refusal) => {
            let mut sector = [0u8; SECTOR_SIZE];
            // WHY re-read LBA 0 for the notice: the refusal path reports the boot
            // area the decision used, and a read failure yields none rather than a
            // fabricated one.
            let boot = if drive.read_sector(0, &mut sector).is_ok() {
                Some(boot_guard::parse_boot_area(&sector))
            } else {
                None
            };
            boot_guard::announce_refusal("persistent journal commit", drive, range, boot.as_ref(), refusal);
            Err(PersistentError::BootDeviceProtected)
        }
    }
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
    // WHY this is an error and not "no record": the magic, the format version and
    // the kind byte all matched, so this slot *was* published by a commit and
    // only its length is unusable. Reporting `Ok(None)` would let a caller
    // conclude that nothing was ever committed in this slot. For the storage
    // root that is the orphaning hazard exactly: `SecureKeystore::recover` reads
    // `Ok(None)` as a first boot and mints a replacement root, orphaning every
    // credential under the real one.
    //
    // `commit_to` can never produce this — it rejects an empty payload and caps
    // at PAYLOAD_CAPACITY — so the record is corrupt or foreign, and the caller
    // has to hear about it. A torn *payload* is different and still recovers as
    // absent: there the length is intact and the tag check below rejects the
    // record while leaving the sibling slot able to answer.
    if len == 0 || len > PAYLOAD_CAPACITY {
        return Err(PersistentError::Corrupt);
    }
    if len > payload.len() {
        // The scratch buffer is a caller-supplied contract, not a property of the
        // record. Both in-tree callers pass PAYLOAD_CAPACITY, so this only fires
        // on a programming error; refusing beats the out-of-range copy below.
        return Err(PersistentError::TooLarge);
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

    if offset != len || record_tag(expected_kind, generation, &payload[..len]) != digest {
        return Ok(None);
    }
    Ok(Some((generation, len)))
}

pub fn load_latest_kind(
    out: &mut [u8],
    kind: u8,
) -> Result<Option<(u64, usize)>, PersistentError> {
    if !(JOURNAL_MIN_KIND..=JOURNAL_MAX_KIND).contains(&kind) {
        return Err(PersistentError::Unavailable);
    }
    let mut guard = ATA_PRIMARY.lock();
    let drive = guard.as_mut().ok_or(PersistentError::Unavailable)?;
    load_latest_from(drive, out, kind)
}

/// Recovery over an explicit device. This is the journal's own recovery
/// routine: it walks every slot, keeps only records whose keyed tag matches
/// their own header fields and full payload, and returns the highest surviving
/// generation. A torn or partially written record is discarded here rather
/// than being handed back half-applied.
fn load_latest_from(
    drive: &mut impl BlockDevice,
    out: &mut [u8],
    kind: u8,
) -> Result<Option<(u64, usize)>, PersistentError> {
    let total = drive.sector_count();

    let mut best: Option<(u64, usize)> = None;
    for slot in 0..TOTAL_SLOT_COUNT {
        let Some(base) = slot_base(total, slot) else { continue; };
        let mut candidate = [0u8; PAYLOAD_CAPACITY];
        if let Some((generation, len)) = read_slot(drive, base, &mut candidate, kind)? {
            // WHY strict `>`: a record is only ever promoted over another one by
            // a strictly newer generation. Re-reading the same generation twice
            // therefore cannot change which bytes come back, which is what makes
            // recovery idempotent.
            if best.map(|(g, _)| generation > g).unwrap_or(true) {
                if len > out.len() {
                    // WHY an error rather than a skip: this record validated, so it
                    // exists. Reporting "no record" to a caller whose buffer was
                    // too small would let it conclude it has nothing to recover and
                    // replace committed state, when in fact it simply failed to read
                    // what was there. The caller must hear about it.
                    return Err(PersistentError::TooLarge);
                }
                out[..len].copy_from_slice(&candidate[..len]);
                best = Some((generation, len));
            }
        }
    }
    Ok(best)
}

/// Authentication key for journal records, published once the device seed
/// exists.
///
/// It lives in its own static rather than being derived from the keystore on
/// demand: the keystore publishes this key while holding its own lock and then
/// commits its seed record, so reading the keystore from inside the commit path
/// would re-enter that lock and deadlock the boot.
static JOURNAL_AUTH_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// Publishes the journal authentication key. Called by the keystore once it has
/// a device seed. Passing a zero key makes record verification fail closed.
pub fn set_journal_auth_key(seed: &[u8; 32]) {
    let key = crate::crypto::hmac::HmacSha256::compute(seed, b"interos.journal.v1");
    *JOURNAL_AUTH_KEY.lock() = Some(key);
}

fn journal_auth_key() -> [u8; 32] {
    match *JOURNAL_AUTH_KEY.lock() {
        Some(key) => key,
        // No key published yet: a zero key makes every tag mismatch rather than
        // silently accepting an unauthenticated record.
        None => [0u8; 32],
    }
}

/// Computes the record authentication tag.
///
/// WHY a keyed MAC rather than a plain hash: an unkeyed digest only proves the
/// payload has not changed, so anyone able to write the data disk could replace
/// the stored trust root, revocation list or storage seed and still pass
/// verification. The tag covers the header fields as well as the payload, so the
/// kind, generation and length cannot be altered either.
///
/// This is a software-backed integrity mechanism. It resists corruption and
/// casual modification; it is not a substitute for a hardware-backed store, and
/// it cannot protect the keystore seed record itself from an attacker who can
/// write the entire disk, because that seed derives this key.
fn record_tag(kind: u8, generation: u64, payload: &[u8]) -> [u8; 32] {
    // The 21 header bytes and the full payload are the MAC input.
    //
    // WHY the whole payload and not a fixed 512-byte window: the write protocol
    // only orders the payload *before* the header, so the tag is the only thing
    // that decides whether a record whose payload was lost, truncated or
    // half-overwritten is believable. A window that stops at byte 512 leaves
    // every record longer than 512 bytes (the VFS snapshot, for one) with an
    // unauthenticated tail, and such a record still validates: a lost sector
    // past the window yields a silently corrupt, half-applied state that the
    // recovery path cannot tell from a good one. Records are capped at
    // PAYLOAD_CAPACITY bytes, so binding the full payload costs one buffer of
    // that size and is what makes "a torn record is discarded" true.
    let mut header = [0u8; 21];
    header[0] = JOURNAL_VERSION as u8;
    header[1] = kind;
    header[2..10].copy_from_slice(&generation.to_le_bytes());
    header[10..14].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    header[14..18].copy_from_slice(&JOURNAL_MAGIC);

    let mut buf = Vec::with_capacity(header.len() + payload.len());
    buf.extend_from_slice(&header);
    buf.extend_from_slice(payload);
    crate::crypto::hmac::HmacSha256::compute(&journal_auth_key(), &buf)
}

/// Maps a record kind to the first of its two journal slots.
///
/// WHY this exists as a function: the declared kinds partition the journal's
/// slots exhaustively, so a kind cannot be added without giving it a pair, and
/// giving it a pair must not move an existing record. Kinds 1..=8 own the
/// primary region; kind 9 retains its historical extension pair and kind 10
/// uses the newly reserved pair below it.
///
/// WHY kind 6 is alone in its own pair and must stay that way: it is the record
/// read under the zero key before the published key exists, so a record
/// committed under the published key could not validate at the moment the key
/// becomes available. Sharing its pair with any other kind would reintroduce
/// exactly that failure.
fn pair_start_for_kind(kind: u8) -> Option<usize> {
    let start = match kind {
        1 => 0usize, 2 => 2usize, 3 => 4usize, 4 => 6usize,
        5 => 8usize, 6 => 10usize, 7 => 12usize, 8 => 14usize,
        9 => 18usize, 10 => 16usize,
        _ => return None,
    };
    Some(start)
}

pub fn commit_kind(payload: &[u8], kind: u8) -> Result<u64, PersistentError> {
    if payload.is_empty() || payload.len() > PAYLOAD_CAPACITY {
        return Err(PersistentError::TooLarge);
    }

    if !(JOURNAL_MIN_KIND..=JOURNAL_MAX_KIND).contains(&kind) {
        return Err(PersistentError::Unavailable);
    }

    let mut guard = ATA_PRIMARY.lock();
    let drive = guard.as_mut().ok_or(PersistentError::Unavailable)?;
    commit_to(drive, payload, kind)
}

/// Commits one record to an explicit device.
fn commit_to(
    drive: &mut impl BlockDevice,
    payload: &[u8],
    kind: u8,
) -> Result<u64, PersistentError> {
    if payload.is_empty() || payload.len() > PAYLOAD_CAPACITY {
        return Err(PersistentError::TooLarge);
    }
    let total = drive.sector_count();
    guard_commit(drive, total)?;

    let pair_start = pair_start_for_kind(kind).ok_or(PersistentError::Unavailable)?;
    let a = pair_start;
    let b = pair_start + 1;
    let mut gen_a = 0u64;
    let base_a = slot_base(total, a as u32).ok_or(PersistentError::Unavailable)?;
    let mut buf = [0u8; PAYLOAD_CAPACITY];
    match read_slot(drive, base_a, &mut buf, kind) {
        Ok(Some((generation, _))) => gen_a = generation,
        Ok(None) | Err(PersistentError::Corrupt) => {}
        Err(e) => return Err(e),
    }

    let mut gen_b = 0u64;
    let base_b = slot_base(total, b as u32).ok_or(PersistentError::Unavailable)?;
    match read_slot(drive, base_b, &mut buf, kind) {
        Ok(Some((generation, _))) => gen_b = generation,
        Ok(None) | Err(PersistentError::Corrupt) => {}
        Err(e) => return Err(e),
    }

    // Keep two slots per record kind. Each kind has an independent alternating
    // pair so a partially written record never replaces the last known-good
    // generation.
    let target = if gen_a <= gen_b { a } else { b };
    let next_generation = gen_a.max(gen_b).saturating_add(1);
    let base = slot_base(total, target as u32).ok_or(PersistentError::Unavailable)?;

    let payload_sectors = ((payload.len() + SECTOR_SIZE - 1) / SECTOR_SIZE) as u32;
    for sector in 1..=payload_sectors {
        let mut buf = [0u8; SECTOR_SIZE];
        let start = ((sector - 1) as usize) * SECTOR_SIZE;
        if start < payload.len() {
            let take = (payload.len() - start).min(SECTOR_SIZE);
            buf[..take].copy_from_slice(&payload[start..start + take]);
        }
        drive.write_sector(base + sector, &buf).map_err(|_| PersistentError::Io)?;
    }

    // WHY the header is written last and is the only thing that publishes the
    // record: a crash anywhere in the payload loop above leaves the previous
    // header in place, so the slot still describes the older generation and its
    // tag no longer matches the overwritten payload. The record is then
    // rejected by `read_slot` and recovery falls back to the untouched slot.
    // A record is therefore either wholly present or wholly absent.
    let tag = record_tag(kind, next_generation, payload);
    let mut header = [0u8; SECTOR_SIZE];
    header[0..4].copy_from_slice(&JOURNAL_MAGIC);
    header[4..8].copy_from_slice(&JOURNAL_VERSION.to_le_bytes());
    header[8..16].copy_from_slice(&next_generation.to_le_bytes());
    header[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    header[20] = kind;
    header[24..56].copy_from_slice(&tag);
    header[56..60].copy_from_slice(&0x494F_534Au32.to_le_bytes());
    drive.write_sector(base, &header).map_err(|_| PersistentError::Io)?;

    Ok(next_generation)
}

/// Backward-compatible default record is the VFS state record (kind 4).
/// New callers should prefer the explicit `*_kind` APIs.
pub fn load_latest(out: &mut [u8]) -> Result<Option<(u64, usize)>, PersistentError> {
    load_latest_kind(out, 4)
}

/// Backward-compatible default record is the VFS state record (kind 4).
/// New callers should prefer the explicit `*_kind` APIs.
pub fn commit(payload: &[u8]) -> Result<u64, PersistentError> {
    commit_kind(payload, 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::block::BlockDeviceError;

    /// Sector-granular in-memory device. A write replaces one sector, which is
    /// the granularity the journal's two-phase commit relies on.
    struct MockDevice {
        sectors: Vec<u8>,
        total: u32,
    }

    impl MockDevice {
        fn new(total: u32) -> Self {
            Self { sectors: alloc::vec![0u8; total as usize * SECTOR_SIZE], total }
        }

        fn base_of(&self, slot: u32) -> u32 {
            slot_base(self.total, slot).expect("mock device must be large enough")
        }

        /// Overwrites one sector of a slot, i.e. a lost or garbled tail write.
        fn scribble_slot_sector(&mut self, slot: u32, sector: u32, fill: u8) {
            let start = (self.base_of(slot) + sector) as usize * SECTOR_SIZE;
            for byte in &mut self.sectors[start..start + SECTOR_SIZE] {
                *byte = fill;
            }
        }

        /// Writes a record payload the way `commit_to` does, but stops before the
        /// header. This is the exact on-disk state a power cut leaves behind.
        fn write_payload_only(&mut self, slot: u32, payload: &[u8]) {
            for sector in 1..SLOT_SECTORS {
                let mut buf = [0u8; SECTOR_SIZE];
                let start = ((sector - 1) as usize) * SECTOR_SIZE;
                if start < payload.len() {
                    let take = (payload.len() - start).min(SECTOR_SIZE);
                    buf[..take].copy_from_slice(&payload[start..start + take]);
                }
                let _ = self.write_sector(self.base_of(slot) + sector, &buf);
            }
        }

        /// Publishes a header that names this kind but whose length is
        /// impossible, leaving the payload whatever it was. `commit_to` can
        /// never write this: it rejects an empty payload and caps at
        /// PAYLOAD_CAPACITY. It is what a corrupt or foreign record looks like.
        fn publish_unreadable_header(&mut self, slot: u32, kind: u8, len: u32) {
            let mut header = [0u8; SECTOR_SIZE];
            header[0..4].copy_from_slice(&JOURNAL_MAGIC);
            header[4..8].copy_from_slice(&JOURNAL_VERSION.to_le_bytes());
            header[8..16].copy_from_slice(&7u64.to_le_bytes());
            header[16..20].copy_from_slice(&len.to_le_bytes());
            header[20] = kind;
            let _ = self.write_sector(self.base_of(slot), &header);
        }

        /// Turns this device into boot media whose partitions cover every sector,
        /// which is the shape of the packaged single-disk image.
        fn install_boot_sector(&mut self) {
            let mut mbr = [0u8; SECTOR_SIZE];
            for (i, (lba, count)) in [(1u32, 961u32), (962, self.total - 962)].iter().enumerate() {
                let o = 446 + i * 16;
                mbr[o] = 0x80;
                mbr[o + 4] = 0x0C;
                mbr[o + 8..o + 12].copy_from_slice(&lba.to_le_bytes());
                mbr[o + 12..o + 16].copy_from_slice(&count.to_le_bytes());
            }
            mbr[510] = 0x55;
            mbr[511] = 0xAA;
            self.sectors[..SECTOR_SIZE].copy_from_slice(&mbr);
        }
    }

    impl BlockDevice for MockDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            let start = lba as usize * SECTOR_SIZE;
            buf.copy_from_slice(&self.sectors[start..start + SECTOR_SIZE]);
            Ok(())
        }

        fn write_sector(&mut self, lba: u32, buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if lba >= self.total { return Err(BlockDeviceError::LbaOutOfRange); }
            let start = lba as usize * SECTOR_SIZE;
            self.sectors[start..start + SECTOR_SIZE].copy_from_slice(buf);
            Ok(())
        }

        fn sector_count(&self) -> u32 { self.total }

        fn model(&self) -> &str { "mock" }
    }

    /// One sector past the journal plus the whole reserved journal region.
    const MOCK_SECTORS: u32 = SLOT_SECTORS * TOTAL_SLOT_COUNT + 1;
    const KIND: u8 = 6;

    fn payload(tag: u8, len: usize) -> Vec<u8> {
        (0..len).map(|i| tag.wrapping_add(i as u8)).collect()
    }

    fn load(dev: &mut MockDevice) -> Option<(u64, usize, Vec<u8>)> {
        let mut out = alloc::vec![0u8; PAYLOAD_CAPACITY];
        match load_latest_from(dev, &mut out, KIND).expect("recovery must not report an error") {
            Some((generation, len)) => {
                let bytes = out[..len].to_vec();
                Some((generation, len, bytes))
            }
            None => None,
        }
    }

    /// A commit to the journal tail of the device the kernel booted from is the
    /// second way the packaged image destroyed itself: the tail reservation puts
    /// the slots inside the FAT boot partition, and every record publishes a
    /// header sector there. Before the guard this wrote the header and payload
    /// straight over the running image.
    #[test]
    fn a_commit_to_the_boot_device_is_refused_and_writes_nothing() {
        let mut dev = MockDevice::new(8192);
        dev.install_boot_sector();
        let before = dev.sectors.clone();

        assert_eq!(
            commit_to(&mut dev, &payload(9, 600), KIND),
            Err(PersistentError::BootDeviceProtected)
        );
        assert_eq!(dev.sectors, before, "a refused commit must leave the device byte-identical");
    }

    /// The same commit on a device with no boot sector still lands, so the guard
    /// has not turned persistence off.
    #[test]
    fn a_commit_to_a_data_device_still_lands() {
        let mut dev = MockDevice::new(8192);
        assert_eq!(commit_to(&mut dev, &payload(9, 600), KIND), Ok(1));
        let (generation, len, bytes) = load(&mut dev).expect("the record must be readable");
        assert_eq!(generation, 1);
        assert_eq!(bytes, payload(9, 600));
        assert_eq!(len, 600);
    }

    /// A refused commit must not look like "nothing was ever committed".
    ///
    /// WHY this matters: `SecureKeystore::recover` reads `Ok(None)` as a first
    /// boot and mints a replacement storage root. If a boot-device refusal were
    /// folded into `Unavailable` or `Ok(None)`, refusing the write would orphan
    /// every credential already committed there.
    #[test]
    fn a_refused_commit_still_reports_its_own_error() {
        assert_ne!(PersistentError::BootDeviceProtected, PersistentError::Unavailable);
        assert_ne!(PersistentError::BootDeviceProtected, PersistentError::Corrupt);
        assert_ne!(PersistentError::BootDeviceProtected, PersistentError::Io);
        assert_ne!(PersistentError::BootDeviceProtected, PersistentError::TooLarge);
    }

    /// A payload sector lost past the old 512-byte tag window used to validate:
    /// the record came back with a corrupt tail and a matching tag, so the
    /// caller could not tell it from a good record. The whole payload is now
    /// inside the MAC, so the record is rejected outright.
    ///
    /// The assertion that proves the defect is fixed: recovery returns `None`
    /// for the only surviving copy instead of `Some(..)` with damaged bytes.
    #[test]
    fn torn_payload_past_the_old_tag_window_is_discarded() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        let slot = pair_start_for_kind(KIND).expect("kind must have a pair") as u32;
        let original = payload(0xA0, 1200);
        assert_eq!(commit_to(&mut dev, &original, KIND), Ok(1));

        // Sector 3 covers bytes 1024..1536 of the payload: outside the window the
        // old tag covered, with the header left intact.
        dev.scribble_slot_sector(slot, 3, 0xEE);

        assert_eq!(
            load(&mut dev),
            None,
            "a record whose payload was torn must be discarded, not returned half-applied",
        );
    }

    /// A tear inside the old window is rejected too; this pins that the fix did
    /// not simply move the boundary.
    #[test]
    fn torn_payload_inside_the_first_window_is_discarded() {
        let slot = pair_start_for_kind(KIND).expect("kind must have a pair") as u32;
        let mut dev = MockDevice::new(MOCK_SECTORS);
        assert_eq!(commit_to(&mut dev, &payload(0x10, 900), KIND), Ok(1));
        dev.scribble_slot_sector(slot, 1, 0x77);
        assert_eq!(load(&mut dev), None);
    }

    /// A committed record that does not fit the caller's buffer must be reported,
    /// not quietly turned into "no record".
    ///
    /// The proving assertion is the error: reporting `Ok(None)` here would let a
    /// caller conclude it has nothing to recover and replace committed state.
    #[test]
    fn surviving_record_that_does_not_fit_the_buffer_is_reported() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        assert_eq!(commit_to(&mut dev, &payload(0x55, 400), KIND), Ok(1));
        let mut small = alloc::vec![0u8; 16];
        assert_eq!(load_latest_from(&mut dev, &mut small, KIND), Err(PersistentError::TooLarge));
    }

    /// The interrupted-commit case: the third commit wrote its payload into the
    /// older slot and died before publishing a header. Recovery must return the
    /// second generation byte for byte, because that is the last state the
    /// system actually committed.
    #[test]
    fn interrupted_commit_keeps_the_last_committed_generation() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        let first = payload(0x01, 1000);
        let second = payload(0x02, 1000);
        let third = payload(0x03, 1000);
        assert_eq!(commit_to(&mut dev, &first, KIND), Ok(1));
        assert_eq!(commit_to(&mut dev, &second, KIND), Ok(2));

        // Generation 3 targets slot 0 (the older one). Simulate the power cut.
        let target = pair_start_for_kind(KIND).expect("kind must have a pair");
        dev.write_payload_only(target as u32, &third);

        let recovered = load(&mut dev).expect("the previous generation must survive");
        assert_eq!(recovered.0, 2, "recovery must fall back to the last committed generation");
        assert_eq!(recovered.2, second, "the recovered payload must be the committed one, byte for byte");
    }

    /// A header sector that was itself torn must not be trusted, and the other
    /// slot must carry recovery.
    #[test]
    fn torn_header_sector_falls_back_to_the_other_slot() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        let first = payload(0x11, 700);
        let second = payload(0x22, 700);
        assert_eq!(commit_to(&mut dev, &first, KIND), Ok(1));
        assert_eq!(commit_to(&mut dev, &second, KIND), Ok(2));

        // Destroy the newest slot's header sector outright.
        dev.scribble_slot_sector(pair_start_for_kind(KIND).expect("pair") as u32 + 1, 0, 0x00);

        let recovered = load(&mut dev).expect("the older intact record must still recover");
        assert_eq!(recovered.0, 1);
        assert_eq!(recovered.2, first);
    }

    /// Recovering twice must equal recovering once: no generation is rewritten,
    /// nothing is promoted, and the same bytes come back.
    #[test]
    fn recovery_is_idempotent() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        assert_eq!(commit_to(&mut dev, &payload(0x31, 1300), KIND), Ok(1));
        let first_pass = load(&mut dev);
        let second_pass = load(&mut dev);
        assert_eq!(first_pass, second_pass);
        assert_eq!(first_pass.map(|(_, _, b)| b), Some(payload(0x31, 1300)));
    }

    /// An empty journal recovers to "no record" rather than to invented data.
    #[test]
    fn absent_record_recovers_as_absent() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        assert_eq!(load(&mut dev), None);
    }

    /// The declared kinds partition every journal slot exhaustively, and each
    /// owns exactly one pair. This is the invariant that makes a kind without a
    /// home visible instead of silently unreachable.
    #[test]
    fn every_declared_kind_owns_one_distinct_slot_pair() {
        let mut seen = [false; TOTAL_SLOT_COUNT as usize];
        for kind in JOURNAL_MIN_KIND..=JOURNAL_MAX_KIND {
            let start = pair_start_for_kind(kind).expect("declared kind must have a pair");
            assert!(start % 2 == 0, "kind {kind} must own an aligned pair");
            assert!(start + 1 < TOTAL_SLOT_COUNT as usize, "kind {kind} pair must fit");
            assert!(!seen[start] && !seen[start + 1], "kind {kind} reuses another kind's pair");
            seen[start] = true;
            seen[start + 1] = true;
        }
        assert!(seen.iter().all(|used| *used), "every journal slot must belong to a kind");
        assert_eq!(pair_start_for_kind(JOURNAL_MAX_KIND + 1), None, "kinds past the range have no pair");
    }

    /// The primary region's slot addresses must not move when a later kind is
    /// given a home in the extension region.
    ///
    /// WHY this is pinned: kind 9 lives below the primary region precisely so
    /// that adding it does not relocate the eight kinds that came before it. If
    /// this ever regresses, every previously committed record becomes
    /// unreachable at its old address and appears to have vanished — the exact
    /// data-loss shape the split exists to prevent.
    #[test]
    fn adding_a_kind_leaves_the_primary_region_where_it_was() {
        const DISK: u32 = 131_072; // the 64 MiB reference image
        for slot in 0..SLOT_COUNT {
            assert_eq!(
                slot_base(DISK, slot),
                Some(DISK - 16 * 128 + slot * 128),
                "primary slot {slot} must keep its historical address",
            );
        }
        // Extension pairs occupy the sectors immediately below the primary base.
        // Kind 10 uses slots 16-17; kind 9 retains slots 18-19.
        assert_eq!(slot_base(DISK, 16), Some(DISK - 20 * 128));
        assert_eq!(slot_base(DISK, 17), Some(DISK - 19 * 128));
        assert_eq!(slot_base(DISK, 18), Some(DISK - 18 * 128));
        assert_eq!(slot_base(DISK, 19), Some(DISK - 17 * 128));
        assert!(slot_base(DISK, 20).is_none(), "there is no slot 20");
    }

    /// Kind 9 — the verified-boot rollback floor — must have a real, durable
    /// home. It used to commit to a kind with no slot, so `advance` could never
    /// succeed and always reported `PersistenceFailed`.
    ///
    /// Pinning assertions: the round trip. The floor is committed, read back
    /// byte for byte, and the second advance is a strictly newer generation.
    #[test]
    fn the_verified_boot_floor_persists_and_survives_an_interrupted_write() {
        const FLOOR: u8 = 9;
        let mut dev = MockDevice::new(MOCK_SECTORS);

        // First advance: nothing is stored, so this is generation 1.
        let floor = 11u64.to_le_bytes();
        assert_eq!(commit_to(&mut dev, &floor, FLOOR), Ok(1), "kind 9 must be committable");

        // Second advance: the older slot is the target, so the first is untouched.
        let raised = 12u64.to_le_bytes();
        assert_eq!(commit_to(&mut dev, &raised, FLOOR), Ok(2));

        let mut out = alloc::vec![0u8; PAYLOAD_CAPACITY];
        let (generation, len) = load_latest_from(&mut dev, &mut out, FLOOR)
            .expect("the floor must be readable")
            .expect("the floor must be present");
        assert_eq!(&out[..len], &raised[..], "the newest floor must come back");
        assert_eq!(generation, 2);

        // A power cut on the third commit must leave the second in place: the
        // rollback floor can never be lost to a torn write, or a bricking update
        // would loop instead of rolling back.
        let interrupted = 13u64.to_le_bytes();
        dev.write_payload_only(pair_start_for_kind(FLOOR).expect("pair") as u32, &interrupted);
        let mut out = alloc::vec![0u8; PAYLOAD_CAPACITY];
        let (generation, len) = load_latest_from(&mut dev, &mut out, FLOOR)
            .expect("recovery must not report an error")
            .expect("the committed floor must survive");
        assert_eq!(&out[..len], &raised[..], "an interrupted advance must not lose the floor");
        assert_eq!(generation, 2);
    }

    /// A committed record this build cannot interpret must be reported, not
    /// folded into "there is no record".
    ///
    /// WHY: the header names this kind, so something *was* committed here. The
    /// length is what cannot be read. Reporting `Ok(None)` lets a caller that
    /// treats absence as "first boot" — which is exactly what
    /// `SecureKeystore::recover` does — mint a replacement root and orphan every
    /// credential stored under the real one.
    ///
    /// Pinning assertions: the two `Err(Corrupt)` results. Both a zero length and
    /// a length past the slot capacity used to return `Ok(None)`.
    #[test]
    fn a_committed_record_with_an_unusable_length_is_reported_not_treated_as_absent() {
        let pair = pair_start_for_kind(KIND).expect("kind must have a pair") as u32;
        for len in [0u32, (PAYLOAD_CAPACITY + 1) as u32] {
            let mut dev = MockDevice::new(MOCK_SECTORS);
            dev.publish_unreadable_header(pair, KIND, len);
            let mut out = alloc::vec![0u8; PAYLOAD_CAPACITY];
            assert_eq!(
                load_latest_from(&mut dev, &mut out, KIND),
                Err(PersistentError::Corrupt),
                "a published record with length {len} must be reported, not read as absent",
            );
        }
    }

    /// A commit is the repair action, so a slot holding an uninterpretable record
    /// is reclaimed rather than blocking every future write of that kind.
    ///
    /// Pinning assertions: `Ok(1)` for the repaired generation and the readable
    /// payload. Refusing here instead would leave a single corrupt slot
    /// permanently wedging the record kind.
    #[test]
    fn a_commit_reclaims_a_slot_holding_an_unreadable_record() {
        let pair = pair_start_for_kind(KIND).expect("kind must have a pair") as u32;
        let mut dev = MockDevice::new(MOCK_SECTORS);
        dev.publish_unreadable_header(pair, KIND, 0);
        assert_eq!(commit_to(&mut dev, &payload(0x44, 300), KIND), Ok(1));
        assert_eq!(load(&mut dev).map(|(_, _, b)| b), Some(payload(0x44, 300)));
    }

    /// A record that exactly fills the slot must survive, and one byte more must
    /// be refused at commit time.
    ///
    /// WHY the second half is the real assertion: `PAYLOAD_CAPACITY` used to
    /// exceed what a slot can physically store by 448 bytes, so a payload in that
    /// band was accepted, published with a valid tag, and then dropped by the read
    /// path — which reported "no record". Committing 65472 bytes of package-store
    /// state would have lost it silently. Refusing is the correct outcome; being
    /// able to store the whole slot is what makes that refusal a bound rather
    /// than a bug.
    #[test]
    fn a_full_slot_round_trips_and_one_byte_more_is_refused() {
        let mut dev = MockDevice::new(MOCK_SECTORS);
        let biggest = payload(0x6B, PAYLOAD_CAPACITY);
        assert_eq!(commit_to(&mut dev, &biggest, KIND), Ok(1));
        assert_eq!(
            load(&mut dev),
            Some((1, PAYLOAD_CAPACITY, biggest)),
            "a record filling the whole slot must read back byte for byte",
        );

        let mut dev = MockDevice::new(MOCK_SECTORS);
        assert_eq!(
            commit_to(&mut dev, &alloc::vec![0u8; PAYLOAD_CAPACITY + 1], KIND),
            Err(PersistentError::TooLarge),
            "a payload larger than the slot must be refused, not accepted and then lost",
        );
        assert_eq!(load(&mut dev), None, "a refused commit must leave the slot empty");
    }
}
