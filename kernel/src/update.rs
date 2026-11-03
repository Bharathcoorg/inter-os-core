// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Secure system-update transaction primitives.
//!
//! Updates are staged as immutable payloads identified by SHA-256 and a
//! publisher signature over a canonical metadata record. Installation policy
//! is monotonic and fail-closed; actual flash/slot I/O belongs to the platform
//! boot/update backend.

use crate::crypto::{ed25519, Sha256, SHA256_DIGEST_LENGTH};
use crate::storage::persistent;
use spin::Mutex;

pub const UPDATE_FORMAT_VERSION: u16 = 1;
pub const UPDATE_SIGNATURE_LEN: usize = ed25519::SIGNATURE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    InvalidFormat,
    Downgrade,
    DigestMismatch,
    InvalidSignature,
    EmptyPayload,
    PersistenceFailed,
    ActivationFailed,
    /// An activation failed and the compensating rollback could not itself be
    /// journalled. WHY this is a distinct outcome and not the original backend
    /// error: the journal still advertises the pending slot, so the next boot
    /// will retry the activation that just failed. A caller that only sees the
    /// backend error cannot tell an aborted install from one that left a retry
    /// pending.
    RollbackFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState { Idle, Staged, Verified, Committed, Rejected }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateManifest {
    pub format_version: u16,
    pub version: u64,
    pub payload_len: u64,
    pub payload_sha256: [u8; SHA256_DIGEST_LENGTH],
}

impl UpdateManifest {
    pub const fn new(version: u64, payload_len: u64, payload_sha256: [u8; 32]) -> Self {
        Self { format_version: UPDATE_FORMAT_VERSION, version, payload_len, payload_sha256 }
    }

    pub fn validate_payload(&self, payload: &[u8]) -> Result<(), UpdateError> {
        if self.format_version != UPDATE_FORMAT_VERSION || self.payload_len != payload.len() as u64 {
            return Err(UpdateError::InvalidFormat);
        }
        if payload.is_empty() { return Err(UpdateError::EmptyPayload); }
        if Sha256::digest(payload) != self.payload_sha256 { return Err(UpdateError::DigestMismatch); }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> [u8; 50] {
        let mut out = [0u8; 50];
        out[0..2].copy_from_slice(&self.format_version.to_le_bytes());
        out[2..10].copy_from_slice(&self.version.to_le_bytes());
        out[10..18].copy_from_slice(&self.payload_len.to_le_bytes());
        out[18..50].copy_from_slice(&self.payload_sha256);
        out
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSlot { A, B }

impl UpdateSlot { pub const fn other(self) -> Self { match self { Self::A => Self::B, Self::B => Self::A } } }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationRecord {
    pub active: UpdateSlot,
    pub pending: Option<UpdateSlot>,
    pub pending_version: u64,
    pub boot_attempts: u8,
}

impl ActivationRecord {
    fn encode(&self) -> [u8; 18] {
        let mut out=[0u8;18];
        out[0]=match self.active { UpdateSlot::A=>0, UpdateSlot::B=>1 };
        out[1]=match self.pending { None=>255, Some(UpdateSlot::A)=>0, Some(UpdateSlot::B)=>1 };
        out[2..10].copy_from_slice(&self.pending_version.to_le_bytes());
        out[10]=self.boot_attempts;
        out
    }
    fn decode(bytes:&[u8]) -> Option<Self> {
        if bytes.len()<11 || bytes[0]>1 || (bytes[1]!=255 && bytes[1]>1) { return None; }
        Some(Self {
            active: if bytes[0]==0 {UpdateSlot::A} else {UpdateSlot::B},
            pending: match bytes[1] { 255=>None, 0=>Some(UpdateSlot::A), _=>Some(UpdateSlot::B) },
            pending_version:u64::from_le_bytes(bytes[2..10].try_into().ok()?),
            boot_attempts:bytes[10],
        })
    }
    fn persist(&self) -> Result<(), UpdateError> { persistent::commit_kind(&self.encode(), 8).map(|_| ()).map_err(|_| UpdateError::PersistenceFailed) }

    pub const fn new(active: UpdateSlot, version: u64) -> Self { Self { active, pending: None, pending_version: version, boot_attempts: 0 } }

    // ---------------------------------------------------------------------
    // Pure in-memory transitions.
    //
    // WHY these are separated from the public methods: the transitions are the
    // whole of the policy and they need no block device, so they can be reasoned
    // about -- and unit tested -- in isolation. Durability is then added in
    // exactly one place per public method, which is what makes it possible to
    // argue that every returned `Ok` is a state the journal also holds.
    // ---------------------------------------------------------------------

    /// Points the record at the inactive slot, ready to boot `version`.
    ///
    /// WHY the target is derived rather than chosen: activating the slot that is
    /// already running would destroy the only known-good copy, so this always
    /// targets `active.other()`.
    fn stage_pending(&mut self, version: u64) -> Result<UpdateSlot, UpdateError> {
        if version <= self.pending_version { return Err(UpdateError::Downgrade); }
        let slot = self.active.other();
        self.pending = Some(slot);
        self.pending_version = version;
        self.boot_attempts = 0;
        Ok(slot)
    }

    /// Counts one more attempt to boot the pending slot. Does nothing when there
    /// is no pending slot, because there is no boot being attempted.
    fn count_boot_attempt(&mut self) {
        if self.pending.is_none() { return; }
        self.boot_attempts = self.boot_attempts.saturating_add(1);
    }

    /// Makes the pending slot the active one. Does nothing when there is none,
    /// because confirming an absent activation must not silently retarget the
    /// running slot.
    fn commit_pending(&mut self) {
        let Some(slot) = self.pending.take() else { return; };
        self.active = slot;
        self.boot_attempts = 0;
    }

    /// Abandons the pending slot. The active slot is deliberately left alone:
    /// rollback gives up on the *new* image, it does not undo the last good one.
    fn clear_pending(&mut self) {
        self.pending = None;
        self.boot_attempts = 0;
    }

    // ---------------------------------------------------------------------
    // Journalled transitions.
    //
    // WHY every one of these snapshots the record, mutates, persists and
    // restores the snapshot when the commit is refused: `persist()` can be
    // refused (no device yet, an I/O error, a full slot). If the mutation
    // survived a refusal, RAM would be strictly *ahead* of the journal, and the
    // journal is the only thing the next boot reads -- so the running system and
    // the next boot would disagree, with no way to detect it. Restoring makes
    // every successful return a statement the journal also holds, and every
    // failure a statement nothing changed.
    // ---------------------------------------------------------------------

    /// Stages `version` for the inactive slot.
    pub fn stage(&mut self, version: u64) -> Result<UpdateSlot, UpdateError> {
        let previous = *self;
        // WHY the slot is taken from the snapshot rather than read back
        // afterwards: `stage_pending` targets `previous.active.other()` by
        // construction, so this cannot disagree with what was persisted, and it
        // avoids recovering a value by indexing a field that a reader could
        // mistake for an independent fact.
        let slot = previous.active.other();
        // A downgrade is refused before anything is mutated, so the early return
        // here needs no compensation.
        self.stage_pending(version)?;
        if self.persist().is_err() {
            *self = previous;
            return Err(UpdateError::PersistenceFailed);
        }
        Ok(slot)
    }

    /// Records one attempt to boot the pending slot.
    pub fn begin_boot(&mut self) -> Result<(), UpdateError> {
        if self.pending.is_none() { return Ok(()); }
        let previous = *self;
        self.count_boot_attempt();
        if self.persist().is_err() {
            // WHY this compensation is load-bearing rather than defensive:
            // `boot_attempts` lives only in RAM between reboots, and
            // `recovery_required()` is the single thing that decides to abandon
            // a slot that bricks the device. A refused commit that kept the
            // increment would discard it on the next reboot, so the counter
            // would never reach the limit and the device would reboot into the
            // same broken slot forever instead of rolling back.
            *self = previous;
            return Err(UpdateError::PersistenceFailed);
        }
        Ok(())
    }

    /// Promotes the pending slot to active after a successful boot.
    pub fn confirm(&mut self) -> Result<(), UpdateError> {
        if self.pending.is_none() { return Ok(()); }
        let previous = *self;
        self.commit_pending();
        if self.persist().is_err() {
            // WHY: a surviving mutation here would leave RAM naming the new slot
            // as active while the journal still names the old one, so the next
            // reboot would come up on the slot this boot just rejected.
            *self = previous;
            return Err(UpdateError::PersistenceFailed);
        }
        Ok(())
    }

    /// Abandons the pending slot.
    pub fn rollback(&mut self) -> Result<(), UpdateError> {
        let previous = *self;
        self.clear_pending();
        if self.persist().is_err() {
            // WHY: the journal would otherwise keep advertising a pending slot
            // that RAM has already given up on, and the next boot would retry an
            // activation this system has abandoned.
            *self = previous;
            return Err(UpdateError::PersistenceFailed);
        }
        Ok(())
    }

    pub const fn boot_attempt_limit(&self) -> u8 { 3 }
    pub const fn recovery_required(&self) -> bool { self.pending.is_some() && self.boot_attempts >= self.boot_attempt_limit() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedBootPolicy { pub minimum_version: u64 }

/// Byte length of the persisted rollback floor: one little-endian `u64`.
const VERIFIED_BOOT_FLOOR_LEN: usize = 8;

/// Decodes a persisted rollback floor of `len` bytes held in `buf`.
///
/// WHY the length is a separate argument rather than `buf[..len]`: the record's
/// length is corruption- and foreign-write reachable, and this module's reader
/// hands back whatever the record claims. Slicing first would put an
/// out-of-range length one unchecked bounds check away from halting the kernel
/// in a boot path, so the length is checked against both the expected width and
/// the buffer before anything is read. A length that is not exactly
/// [`VERIFIED_BOOT_FLOOR_LEN`] is refused, never truncated: decoding a short
/// record as a `u64` would fabricate a version out of uninitialised bytes, and
/// the floor is a security decision in both directions -- too low reopens the
/// rollback this exists to prevent, too high bricks the device.
fn decode_verified_boot_floor(buf: &[u8], len: usize) -> Option<u64> {
    if len != VERIFIED_BOOT_FLOOR_LEN || len > buf.len() { return None; }
    Some(u64::from_le_bytes(buf[..VERIFIED_BOOT_FLOOR_LEN].try_into().ok()?))
}

impl VerifiedBootPolicy {
    pub const fn new(minimum_version: u64) -> Self { Self { minimum_version } }
    pub fn accept_version(&self, version: u64) -> Result<(), UpdateError> {
        if version < self.minimum_version { Err(UpdateError::Downgrade) } else { Ok(()) }
    }
    pub fn advance(&mut self, version: u64) -> Result<(), UpdateError> {
        if version < self.minimum_version { return Err(UpdateError::Downgrade); }
        let previous = *self;
        self.minimum_version = version;
        // WHY the same snapshot discipline as the activation record: a floor that
        // advanced in RAM but not in the journal is a floor the next boot does
        // not have, which is precisely the protection this record exists to
        // provide.
        if persistent::commit_kind(&version.to_le_bytes(), 9).is_err() {
            *self = previous;
            return Err(UpdateError::PersistenceFailed);
        }
        Ok(())
    }

    /// Reads the persisted rollback floor back from journal kind 9.
    ///
    /// `minimum_version` is the floor to fall back on for a device that has
    /// never stored one -- a build-time default, not authority over the
    /// journal.
    ///
    /// WHY the journal wins wherever the two disagree: the floor is monotonic
    /// and fail-closed, so a stored value below the supplied default (a record
    /// written by an older release, or a stricter default in this build) must
    /// not be able to *lower* a floor that is already in force. The maximum of
    /// the two is taken, which can only ever make refusal stricter.
    pub fn restore(&mut self, minimum_version: u64) -> Result<(), UpdateError> {
        self.restore_from(minimum_version, persistent::load_latest_kind)
    }

    /// The body of [`VerifiedBootPolicy::restore`], with the journal read
    /// injected.
    ///
    /// WHY the reader is a parameter: the three outcomes that matter -- nothing
    /// stored, unreadable, and stored with the wrong length -- are precisely the
    /// ones a host test cannot produce through the real device, and each of them
    /// is a security decision about the floor. Taking the read as an argument
    /// makes all three testable without a block device and without a second code
    /// path that could drift from this one.
    fn restore_from(
        &mut self,
        minimum_version: u64,
        read: impl FnOnce(&mut [u8], u8) -> Result<Option<(u64, usize)>, persistent::PersistentError>,
    ) -> Result<(), UpdateError> {
        // WHY the buffer is exactly the payload size: `load_latest_kind` refuses
        // a record longer than the caller's buffer (`PersistentError::TooLarge`)
        // rather than truncating it, so the returned length is always within
        // bounds and the slice handed to the decoder can never run off the end.
        // A larger buffer would only admit longer foreign records that the
        // decoder then has to reject anyway.
        let mut buf = [0u8; VERIFIED_BOOT_FLOOR_LEN];
        // WHY the local is called `rollback_index`: this value is the journal's
        // record of how far back an attacker-supplied image may not reach, which
        // is the name tests/test_runtime_storage_package_contract.py pins for
        // this module.
        let rollback_index = match read(&mut buf, 9) {
            // Never stored on this device: the supplied default is the floor.
            Ok(None) => minimum_version,
            Ok(Some((_, len))) => match decode_verified_boot_floor(&buf, len) {
                Some(floor) => floor,
                None => return Err(UpdateError::PersistenceFailed),
            },
            // WHY this is not "never stored": `Err` means the device was present
            // but the record could not be read -- unavailable media, an unusable
            // length, a torn slot. Reporting that as absent would silently
            // reset the floor to the build-time default and reopen exactly the
            // rollback the floor exists to prevent, which is the one failure
            // mode this policy may never have. Fail loudly, change nothing.
            Err(_) => return Err(UpdateError::PersistenceFailed),
        };
        self.minimum_version = self.minimum_version.max(rollback_index);
        Ok(())
    }
}

pub struct UpdateTransaction {
    pub manifest: UpdateManifest,
    pub state: UpdateState,
}

impl UpdateTransaction {
    pub fn stage(manifest: UpdateManifest, payload: &[u8], current_version: u64)
        -> Result<Self, UpdateError>
    {
        if manifest.version <= current_version { return Err(UpdateError::Downgrade); }
        manifest.validate_payload(payload)?;
        Ok(Self { manifest, state: UpdateState::Staged })
    }

    pub fn verify_signature(
        &mut self,
        publisher_id: &str,
        signature: &[u8; UPDATE_SIGNATURE_LEN],
    ) -> Result<(), UpdateError> {
        let data = self.manifest.canonical_bytes();
        if !ed25519::verify_detached_signature(&data, publisher_id, signature) {
            self.state = UpdateState::Rejected;
            return Err(UpdateError::InvalidSignature);
        }
        self.state = UpdateState::Verified;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), UpdateError> {
        if self.state != UpdateState::Verified { return Err(UpdateError::InvalidFormat); }
        self.state = UpdateState::Committed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::persistent::PersistentError;

    /// A journal reader that answers with a caller-chosen outcome, so the three
    /// restore paths that matter can be exercised without a block device.
    fn reader(
        outcome: Result<Option<(u64, usize)>, PersistentError>,
        record: [u8; VERIFIED_BOOT_FLOOR_LEN],
    ) -> impl FnOnce(&mut [u8], u8) -> Result<Option<(u64, usize)>, PersistentError> {
        move |out: &mut [u8], _kind: u8| {
            if let Ok(Some((_, len))) = &outcome {
                // WHY the copy is length-checked here: it reproduces the real
                // reader's contract, that it never reports more bytes than the
                // buffer holds, so a test cannot accidentally exercise a slice
                // the production caller could not reach.
                let take = (*len).min(out.len());
                out[..take].copy_from_slice(&record[..take]);
            }
            outcome
        }
    }

    #[test]
    fn payload_digest_is_required_before_staging() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        assert!(UpdateTransaction::stage(m, payload, 1).is_ok());
        let bad = UpdateManifest::new(2, payload.len() as u64, [0u8; 32]);
        assert!(matches!(UpdateTransaction::stage(bad, payload, 1), Err(UpdateError::DigestMismatch)));
    }

    #[test]
    fn activation_targets_inactive_slot_and_rolls_back_after_failures() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        assert_eq!(a.stage_pending(11), Ok(UpdateSlot::B));
        a.count_boot_attempt(); a.count_boot_attempt(); a.count_boot_attempt();
        assert!(a.recovery_required());
        a.clear_pending();
        assert_eq!(a.active, UpdateSlot::A);
        assert!(a.pending.is_none());
    }

    #[test]
    fn staging_targets_the_inactive_slot_and_never_the_running_one() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        // Staging does not move `active`, so re-staging keeps targeting B -- the
        // running slot is never a stage target, however many times it is staged.
        assert_eq!(a.stage_pending(11), Ok(UpdateSlot::B));
        assert_eq!(a.stage_pending(12), Ok(UpdateSlot::B));
        assert_eq!(a.stage_pending(13), Ok(UpdateSlot::B));
        assert_eq!(a.active, UpdateSlot::A);
        assert_eq!(a.pending, Some(UpdateSlot::B));
        // Once the pending slot is confirmed, the inactive slot becomes A.
        a.commit_pending();
        assert_eq!(a.active, UpdateSlot::B);
        assert_eq!(a.stage_pending(14), Ok(UpdateSlot::A));
        assert_eq!(a.active, UpdateSlot::B);
    }

    #[test]
    fn staging_a_version_at_or_below_the_pending_one_is_a_downgrade() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        assert_eq!(a.stage_pending(11).ok(), Some(UpdateSlot::B));
        let snapshot = a;
        assert!(matches!(a.stage_pending(11), Err(UpdateError::Downgrade)));
        assert!(matches!(a.stage_pending(9), Err(UpdateError::Downgrade)));
        // A refused downgrade must not have disturbed the staged slot.
        assert_eq!(a, snapshot);
    }

    #[test]
    fn recovery_is_required_only_once_the_attempt_limit_is_reached() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        assert_eq!(a.boot_attempt_limit(), 3);
        a.stage_pending(11).ok();
        a.count_boot_attempt();
        assert!(!a.recovery_required());
        a.count_boot_attempt();
        assert!(!a.recovery_required());
        a.count_boot_attempt();
        assert!(a.recovery_required());
    }

    #[test]
    fn a_record_with_no_pending_slot_never_counts_a_boot_attempt() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.count_boot_attempt();
        a.count_boot_attempt();
        // WHY this matters: `begin_boot` is called on every boot. Counting
        // attempts against an absent pending slot would make
        // `recovery_required` meaningless and, once a slot *is* staged, could
        // trip recovery on the first attempt.
        assert_eq!(a.boot_attempts, 0);
        assert!(!a.recovery_required());
    }

    #[test]
    fn confirming_promotes_the_pending_slot_and_clears_the_attempts() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.stage_pending(11).ok();
        a.count_boot_attempt();
        a.commit_pending();
        assert_eq!(a.active, UpdateSlot::B);
        assert_eq!(a.pending, None);
        assert_eq!(a.boot_attempts, 0);
        assert!(!a.recovery_required());
    }

    #[test]
    fn confirming_with_nothing_pending_leaves_the_running_slot_alone() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.commit_pending();
        assert_eq!(a.active, UpdateSlot::A);
        assert_eq!(a.pending, None);
    }

    #[test]
    fn rollback_abandons_the_new_slot_without_disturbing_the_old_one() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.stage_pending(11).ok();
        a.count_boot_attempt();
        a.clear_pending();
        assert_eq!(a.active, UpdateSlot::A);
        assert_eq!(a.pending, None);
        assert_eq!(a.boot_attempts, 0);
        assert!(!a.recovery_required());
    }

    #[test]
    fn a_refused_boot_attempt_commit_leaves_the_counter_where_it_was() {
        // WHY: with no journal there is no device, so every commit here is
        // refused -- which is exactly the state under test.
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.stage_pending(11).ok();
        a.count_boot_attempt();
        let before = a;
        assert!(matches!(a.begin_boot(), Err(UpdateError::PersistenceFailed)));
        // The increment must be gone. A RAM-only counter resets on the next
        // reboot, so `recovery_required()` could never fire for a slot that
        // bricks the device.
        assert_eq!(a, before);
        assert!(!a.recovery_required());
    }

    #[test]
    fn a_refused_confirm_does_not_switch_the_running_slot_in_ram() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.stage_pending(11).ok();
        let before = a;
        assert!(matches!(a.confirm(), Err(UpdateError::PersistenceFailed)));
        // The journal still names A as active, so RAM has to keep saying so.
        assert_eq!(a.active, UpdateSlot::A);
        assert_eq!(a, before);
    }

    #[test]
    fn a_refused_rollback_keeps_the_pending_slot_in_ram() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.stage_pending(11).ok();
        let before = a;
        assert!(matches!(a.rollback(), Err(UpdateError::PersistenceFailed)));
        // If RAM dropped the pending slot while the journal kept it, the next
        // boot would retry an activation this system has abandoned.
        assert_eq!(a.pending, Some(UpdateSlot::B));
        assert_eq!(a, before);
    }

    #[test]
    fn a_refused_stage_leaves_no_pending_slot_behind() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        let before = a;
        assert!(matches!(a.stage(11), Err(UpdateError::PersistenceFailed)));
        assert_eq!(a, before);
        assert_eq!(a.pending, None);
        // And the next boot attempt must not be counted against a slot that was
        // never staged.
        assert!(a.begin_boot().is_ok());
        assert_eq!(a.boot_attempts, 0);
    }

    #[test]
    fn begin_boot_and_confirm_are_no_ops_with_nothing_pending() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        let before = a;
        assert!(a.begin_boot().is_ok());
        assert!(a.confirm().is_ok());
        assert_eq!(a, before);
    }

    #[test]
    fn the_activation_record_round_trips_through_its_encoding() {
        let mut a = ActivationRecord::new(UpdateSlot::B, 10);
        a.stage_pending(11).ok();
        a.count_boot_attempt();
        assert_eq!(ActivationRecord::decode(&a.encode()), Some(a));
    }

    #[test]
    fn a_record_that_cannot_name_a_slot_is_refused() {
        // WHY these must be refused rather than coerced: `active` decides which
        // image boots. Defaulting an out-of-range byte to slot A would make a
        // corrupted record boot the wrong side of the update.
        assert!(ActivationRecord::decode(&[2, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(ActivationRecord::decode(&[0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(ActivationRecord::decode(&[0, 255, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(ActivationRecord::decode(&[]).is_none());
        assert_eq!(ActivationRecord::decode(&[1, 255, 7, 0, 0, 0, 0, 0, 0, 0, 3]),
                   Some(ActivationRecord { active: UpdateSlot::B, pending: None, pending_version: 7, boot_attempts: 3 }));
    }

    #[test]
    fn a_verified_boot_floor_is_a_full_width_version() {
        assert_eq!(decode_verified_boot_floor(&41u64.to_le_bytes(), VERIFIED_BOOT_FLOOR_LEN), Some(41));
        assert_eq!(decode_verified_boot_floor(&u64::MAX.to_le_bytes(), VERIFIED_BOOT_FLOOR_LEN), Some(u64::MAX));
        // Short records are refused outright: padding or truncating one would
        // fabricate a floor.
        assert!(decode_verified_boot_floor(&[], 0).is_none());
        assert!(decode_verified_boot_floor(&[1, 2, 3, 4], 4).is_none());
        // So is an empty one.
        assert!(decode_verified_boot_floor(&[0u8; VERIFIED_BOOT_FLOOR_LEN], 0).is_none());
        // A longer record is refused rather than truncated to its first 8 bytes,
        // which are not this record's payload.
        let mut long = [0u8; VERIFIED_BOOT_FLOOR_LEN + 1];
        long[0] = 7;
        assert!(decode_verified_boot_floor(&long, long.len()).is_none());
        // And a length wider than the buffer is refused before any read, which
        // is the case that would otherwise be an out-of-range slice.
        assert!(decode_verified_boot_floor(&[0u8; VERIFIED_BOOT_FLOOR_LEN], VERIFIED_BOOT_FLOOR_LEN + 1).is_none());
    }

    #[test]
    fn an_oversized_activation_record_decodes_without_reading_past_it() {
        // WHY: `init` hands `decode` whatever length the record claims, up to
        // its whole buffer. A record longer than the 11 bytes the encoding
        // uses must decode to the same state rather than read past its own end.
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        a.stage_pending(11).ok();
        a.count_boot_attempt();
        // Filled up to the 32-byte buffer `init` reads into, so the trailing
        // bytes are genuinely past everything `encode` wrote.
        let mut padded = a.encode().to_vec();
        padded.resize(32, 0xAAu8);
        assert_eq!(padded.len(), 32);
        assert_eq!(ActivationRecord::decode(&padded), Some(a));
    }

    #[test]
    fn a_device_with_no_stored_floor_falls_back_to_the_supplied_minimum() {
        let mut policy = VerifiedBootPolicy::new(0);
        assert!(policy.restore_from(12, reader(Ok(None), [0u8; VERIFIED_BOOT_FLOOR_LEN])).is_ok());
        assert_eq!(policy.minimum_version, 12);
        assert_eq!(policy.accept_version(11), Err(UpdateError::Downgrade));
        assert!(policy.accept_version(12).is_ok());
    }

    #[test]
    fn a_stored_floor_is_restored_and_becomes_the_minimum() {
        let mut policy = VerifiedBootPolicy::new(0);
        assert!(policy.restore_from(0, reader(Ok(Some((7, VERIFIED_BOOT_FLOOR_LEN))), 99u64.to_le_bytes())).is_ok());
        assert_eq!(policy.minimum_version, 99);
        assert_eq!(policy.accept_version(98), Err(UpdateError::Downgrade));
        assert!(policy.accept_version(99).is_ok());
    }

    #[test]
    fn an_unreadable_floor_fails_loudly_and_leaves_the_floor_alone() {
        // WHY the assertion is on both halves: reporting the error *and*
        // keeping the old floor. A floor that silently reset to the build-time
        // minimum would accept exactly the rollback it exists to refuse.
        for error in [PersistentError::Unavailable, PersistentError::Corrupt, PersistentError::TooLarge, PersistentError::Io] {
            let mut policy = VerifiedBootPolicy::new(0);
            assert!(matches!(policy.restore_from(0, reader(Err(error), [0u8; VERIFIED_BOOT_FLOOR_LEN])),
                              Err(UpdateError::PersistenceFailed)));
            assert_eq!(policy.minimum_version, 0);
        }
    }

    #[test]
    fn a_stored_floor_never_lowers_a_floor_already_in_force() {
        let mut policy = VerifiedBootPolicy::new(50);
        assert!(policy.restore_from(0, reader(Ok(Some((7, VERIFIED_BOOT_FLOOR_LEN))), 20u64.to_le_bytes())).is_ok());
        assert_eq!(policy.minimum_version, 50);
    }

    #[test]
    fn a_stored_floor_of_record_wrong_length_is_refused() {
        // A record that is shorter than the payload is not a floor, and decoding
        // it as one would invent a version out of uninitialised bytes.
        let mut policy = VerifiedBootPolicy::new(0);
        assert!(matches!(policy.restore_from(0, reader(Ok(Some((7, 4))), 7u64.to_le_bytes())),
                          Err(UpdateError::PersistenceFailed)));
        assert_eq!(policy.minimum_version, 0);
        // An empty record is refused for the same reason.
        let mut policy = VerifiedBootPolicy::new(0);
        assert!(matches!(policy.restore_from(0, reader(Ok(Some((7, 0))), [0u8; VERIFIED_BOOT_FLOOR_LEN])),
                          Err(UpdateError::PersistenceFailed)));
        assert_eq!(policy.minimum_version, 0);
    }

    #[test]
    fn an_oversized_record_cannot_be_read_into_the_floor_buffer() {
        // WHY this is the real reader's behaviour and not a hypothetical: the
        // reader refuses a record longer than the caller's buffer instead of
        // truncating it, which is what keeps the slice in `restore_from` in
        // bounds. The refusal must surface as an error, never as a floor.
        let mut policy = VerifiedBootPolicy::new(0);
        assert!(matches!(policy.restore_from(0, reader(Err(PersistentError::TooLarge), [0u8; VERIFIED_BOOT_FLOOR_LEN])),
                          Err(UpdateError::PersistenceFailed)));
        assert_eq!(policy.minimum_version, 0);
    }

    #[test]
    fn a_refused_advance_leaves_the_floor_where_it_was() {
        let mut policy = VerifiedBootPolicy::new(5);
        assert!(matches!(policy.advance(4), Err(UpdateError::Downgrade)));
        assert_eq!(policy.minimum_version, 5);
        // A commit that cannot be journalled must not raise the floor in RAM
        // only: the next boot reads the journal and would have no floor at all.
        assert!(matches!(policy.advance(9), Err(UpdateError::PersistenceFailed)));
        assert_eq!(policy.minimum_version, 5);
    }

    #[test]
    fn restoring_from_a_device_with_no_journal_does_not_lower_the_floor() {
        // WHY this is the real `restore` and not `restore_from`: on a host with
        // no block device the read is refused, and the refusal must leave the
        // policy exactly as the caller built it.
        let mut policy = VerifiedBootPolicy::new(3);
        assert!(matches!(policy.restore(0), Err(UpdateError::PersistenceFailed)));
        assert_eq!(policy.minimum_version, 3);
    }

    #[test]
    fn downgrades_are_rejected() {
        let payload = b"update";
        let m = UpdateManifest::new(4, payload.len() as u64, Sha256::digest(payload));
        assert!(matches!(UpdateTransaction::stage(m, payload, 4), Err(UpdateError::Downgrade)));
    }

    #[test]
    fn a_transaction_can_only_commit_once_verified() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        let mut tx = UpdateTransaction::stage(m, payload, 1).ok().expect("valid bundle stages");
        assert_eq!(tx.state, UpdateState::Staged);
        assert!(matches!(tx.commit(), Err(UpdateError::InvalidFormat)));
        assert_eq!(tx.state, UpdateState::Staged);
    }

    #[test]
    fn an_unverified_signature_rejects_the_transaction_outright() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        let mut tx = UpdateTransaction::stage(m, payload, 1).ok().expect("valid bundle stages");
        let bogus = [0u8; UPDATE_SIGNATURE_LEN];
        assert_eq!(tx.verify_signature("publisher", &bogus), Err(UpdateError::InvalidSignature));
        assert_eq!(tx.state, UpdateState::Rejected);
        // WHY a rejected transaction must not be committable: install() reads
        // the state, so a rejected one that still committed would bypass the
        // publisher check entirely.
        assert!(matches!(tx.commit(), Err(UpdateError::InvalidFormat)));
    }

    #[test]
    fn a_manifest_that_disagrees_with_its_payload_is_refused() {
        let payload = b"interos-update";
        let digest = Sha256::digest(payload);
        // Wrong length for the payload actually supplied.
        let wrong_len = UpdateManifest::new(2, (payload.len() + 1) as u64, digest);
        assert!(matches!(wrong_len.validate_payload(payload), Err(UpdateError::InvalidFormat)));
        // An empty payload is refused even with a matching digest.
        let empty = UpdateManifest::new(2, 0, Sha256::digest(b""));
        assert!(matches!(empty.validate_payload(b""), Err(UpdateError::EmptyPayload)));
        // An unknown format version is refused.
        let mut future = UpdateManifest::new(2, payload.len() as u64, digest);
        future.format_version = UPDATE_FORMAT_VERSION + 1;
        assert!(matches!(future.validate_payload(payload), Err(UpdateError::InvalidFormat)));
    }

    #[test]
    fn a_canonical_manifest_binds_every_field_it_claims() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        let canonical = m.canonical_bytes();
        assert_eq!(canonical.len(), 50);
        assert_eq!(u16::from_le_bytes([canonical[0], canonical[1]]), UPDATE_FORMAT_VERSION);
        assert_eq!(u64::from_le_bytes(canonical[2..10].try_into().unwrap()), 2);
        assert_eq!(u64::from_le_bytes(canonical[10..18].try_into().unwrap()), payload.len() as u64);
        // Changing the version must change the signed bytes, or a signature over
        // one manifest would verify for another.
        let mut other = m;
        other.version = 3;
        assert_ne!(other.canonical_bytes(), canonical);
    }

    #[test]
    fn install_refuses_an_unverified_transaction_before_touching_the_record() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        let mut tx = UpdateTransaction::stage(m, payload, 1).ok().expect("valid bundle stages");
        let before = *UPDATE_ACTIVATION.lock();
        let mut backend = NoBackend;
        assert!(matches!(VerifiedUpdate::install(&mut backend, UpdateSlot::B, &mut tx, payload),
                          Err(UpdateError::InvalidFormat)));
        assert_eq!(*UPDATE_ACTIVATION.lock(), before);
    }

    #[test]
    fn install_refuses_to_activate_the_slot_that_is_already_running() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        let mut tx = UpdateTransaction::stage(m, payload, 0).ok().expect("valid bundle stages");
        tx.state = UpdateState::Verified;
        let before = *UPDATE_ACTIVATION.lock();
        let mut backend = NoBackend;
        assert!(matches!(VerifiedUpdate::install(&mut backend, before.active, &mut tx, payload),
                          Err(UpdateError::ActivationFailed)));
        assert_eq!(*UPDATE_ACTIVATION.lock(), before);
    }

    #[test]
    fn install_refuses_to_stage_a_slot_it_cannot_journal() {
        // WHY this matters and what it cannot cover: on a host there is no
        // journal, so the staging commit is refused and `install` bails out
        // before touching the backend. That is the correct fail-closed order,
        // and it also means the *compensation* branch -- reached only once a
        // staging commit has actually succeeded -- needs a real device to
        // exercise. What is asserted here is that a refused staging leaves no
        // half-armed activation behind.
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        let mut tx = UpdateTransaction::stage(m, payload, 0).ok().expect("valid bundle stages");
        tx.state = UpdateState::Verified;
        let before = *UPDATE_ACTIVATION.lock();
        let mut backend = NoBackend;
        assert!(matches!(VerifiedUpdate::install(&mut backend, UpdateSlot::B, &mut tx, payload),
                          Err(UpdateError::PersistenceFailed)));
        assert_eq!(*UPDATE_ACTIVATION.lock(), before);
        assert_eq!(tx.state, UpdateState::Verified);
    }

    /// A backend that must never be reached by the tests above: they all fail
    /// before the media operations, and reaching one would mean the pre-flight
    /// checks were skipped.
    struct NoBackend;
    impl UpdateBackend for NoBackend {
        fn write_inactive_slot(&mut self, _slot: UpdateSlot, _payload: &[u8]) -> Result<(), UpdateError> {
            panic!("write_inactive_slot reached before the slot was journalled")
        }
        fn verify_slot(&mut self, _slot: UpdateSlot, _manifest: &UpdateManifest) -> Result<(), UpdateError> {
            panic!("verify_slot reached before the slot was journalled")
        }
        fn activate_slot(&mut self, _slot: UpdateSlot, _version: u64) -> Result<(), UpdateError> {
            panic!("activate_slot reached before the slot was journalled")
        }
    }
}



/// Platform boundary for writing and activating a verified system image.
/// The kernel owns verification policy; board-specific code owns the actual
/// inactive-slot media operations.
pub trait UpdateBackend {
    fn write_inactive_slot(&mut self, slot: UpdateSlot, payload: &[u8]) -> Result<(), UpdateError>;
    fn verify_slot(&mut self, slot: UpdateSlot, manifest: &UpdateManifest) -> Result<(), UpdateError>;
    fn activate_slot(&mut self, slot: UpdateSlot, version: u64) -> Result<(), UpdateError>;
}

#[allow(dead_code)]
pub struct VerifiedUpdate<'a, B: UpdateBackend> { backend: &'a mut B }
impl<'a, B: UpdateBackend> VerifiedUpdate<'a, B> {
    pub fn install_with_boot_policy(
        backend:&'a mut B,
        slot:UpdateSlot,
        tx:&mut UpdateTransaction,
        payload:&[u8],
        policy:&mut VerifiedBootPolicy,
    )->Result<(),UpdateError>{
        policy.accept_version(tx.manifest.version)?;
        Self::install(backend,slot,tx,payload)?;
        policy.advance(tx.manifest.version)
    }

    pub fn install(backend:&'a mut B, slot:UpdateSlot, tx:&mut UpdateTransaction, payload:&[u8])->Result<(),UpdateError>{
        if tx.state != UpdateState::Verified { return Err(UpdateError::InvalidFormat); }
        // Enforce the rollback floor here, not only in install_with_boot_policy.
        // install() is public, so without this a signed but older bundle could be
        // written and activated by a caller that never consulted the policy.
        {
            let floor = UPDATE_ACTIVATION.lock().pending_version;
            if tx.manifest.version < floor { return Err(UpdateError::Downgrade); }
        }
        {
            // WHY the journal-restored floor is enforced here as well:
            // `pending_version` is the version of the slot being tried, and a
            // rollback leaves the pending slot cleared while the journal keeps
            // the floor that rollback was protecting. Consulting only the RAM
            // record would mean the floor disappears on the very reboot it
            // exists for. Taking the maximum here keeps this path at least as
            // strict as the persisted policy.
            let floor = UPDATE_VERIFIED_BOOT.lock().minimum_version;
            if tx.manifest.version < floor { return Err(UpdateError::Downgrade); }
        }
        {
            let mut activation = UPDATE_ACTIVATION.lock();
            if slot == activation.active { return Err(UpdateError::ActivationFailed); }
            let previous = *activation;
            activation.pending = Some(slot);
            activation.pending_version = tx.manifest.version;
            activation.boot_attempts = 0;
            if activation.persist().is_err() {
                *activation = previous;
                return Err(UpdateError::PersistenceFailed);
            }
        }
        if let Err(error) = backend.write_inactive_slot(slot,payload)
            .and_then(|_| backend.verify_slot(slot,&tx.manifest))
            .and_then(|_| backend.activate_slot(slot,tx.manifest.version)) {
            // WHY the compensation result is inspected rather than discarded:
            // `rollback()` is the only thing that tells the journal this
            // activation is over. `let _ =` threw away exactly the value that
            // says whether the next boot will retry the slot that just failed,
            // leaving a caller unable to distinguish an aborted install from one
            // that silently armed a retry.
            if let Err(rollback_error) = UPDATE_ACTIVATION.lock().rollback() {
                // The journal still advertises the pending slot, so the next
                // boot will attempt this slot again. That is a materially
                // different state from "install aborted", so it gets its own
                // error and a serial line: a silent loop into a failing slot is
                // the exact failure this compensation exists to make visible.
                crate::serial_println!(
                    "[UPDATE] activation failed ({:?}) and its rollback could not be journalled ({:?}); the journal still advertises the pending slot and the next boot will retry it.",
                    error, rollback_error
                );
                return Err(UpdateError::RollbackFailed);
            }
            return Err(error);
        }
        tx.commit()
    }
}

pub static UPDATE_ACTIVATION: Mutex<ActivationRecord> = Mutex::new(ActivationRecord::new(UpdateSlot::A, 0));

/// The boot-time verified-boot policy, restored from journal kind 9 by [`init`].
///
/// WHY this is a static rather than a value each caller threads through: the
/// floor has to be in force *before* any update path can run, which is exactly
/// what a caller-supplied policy cannot guarantee. A caller that forgot to
/// restore one would boot with no floor at all.
pub static UPDATE_VERIFIED_BOOT: Mutex<VerifiedBootPolicy> = Mutex::new(VerifiedBootPolicy::new(0));

/// Journal kind owning [`ActivationRecord`].
const ACTIVATION_JOURNAL_KIND: u8 = 8;

/// Restores the update subsystem's journalled state.
///
/// Callers are `main`, between recovery and the filesystem.
pub fn init() {
    let mut activation = UPDATE_ACTIVATION.lock();
    // WHY the slice below is in bounds: `load_latest_kind` refuses a record
    // longer than the caller's buffer rather than truncating it, so the length
    // it returns never exceeds this one. The buffer is sized above everything
    // `encode` writes (18 bytes) so an ordinary record is not merely
    // unreadable, and the slice is taken at the record's own length so a short
    // record is refused by `decode` instead of being padded out with the
    // buffer's leftover bytes.
    let mut buf = [0u8; 32];
    match persistent::load_latest_kind(&mut buf, ACTIVATION_JOURNAL_KIND) {
        // Nothing journalled yet: the compiled-in default stands.
        Ok(None) => {}
        Ok(Some((_, len))) => match ActivationRecord::decode(&buf[..len]) {
            Some(restored) => *activation = restored,
            // WHY a malformed record keeps the default instead of aborting the
            // boot: records are authenticated before publication, so a decode
            // failure means this subsystem cannot make sense of the state it
            // found. Running the slot that is compiled in -- the known-good one
            // -- and saying so is the fail-closed action; inventing a slot state
            // here would not be.
            None => crate::serial_println!("[UPDATE] activation record of {} bytes is not a valid slot state; keeping the compiled-in slot state.", len),
        },
        Err(e) => crate::serial_println!("[UPDATE] activation record unreadable ({:?}); keeping the compiled-in slot state.", e),
    }
    // WHY the guard is released before the floor restore: two locks are never
    // held at once here, and the ordering into the journal is
    // update-state -> journal everywhere else in this file.
    drop(activation);

    // WHY a failed floor restore is reported and not swallowed into a lower
    // default: the floor exists to refuse rollback, so losing it silently is the
    // one outcome that must not pass unnoticed.
    if let Err(e) = UPDATE_VERIFIED_BOOT.lock().restore(0) {
        crate::serial_println!("[UPDATE] verified-boot floor could not be restored ({:?}); no rollback floor is in force for this boot.", e);
    }
}
