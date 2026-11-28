// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The A/B update engine: the ordering rules that make an update durable.
//!
//! THE ONE RULE EVERY METHOD HERE IS BUILT ON: the durable state is changed
//! first and the in-memory state second. A method computes the state it wants
//! to publish, writes it to the device, and adopts it in memory only after the
//! write has succeeded and been read back. When a write fails, the in-memory
//! state is re-read from the device, so the two can never disagree: the
//! engine reports exactly what the media holds, and the caller is told the
//! operation failed.
//!
//! WHAT THAT BUYS, IN THE WORDS OF THE FAILURE CASES THIS SUBSYSTEM HAS TO
//! SURVIVE:
//!
//! * A promotion whose commit fails leaves the slot exactly as it was. The
//!   slot is not marked valid, so it cannot be booted as a confirmed image.
//!   The engine then tries to record the slot as unverified, which is the
//!   fail-safe direction: it can only stop a boot, never start one.
//! * A boot failure whose commit fails does not silently decrement a counter
//!   that was never written. The error is returned, and the in-memory state
//!   is re-read from the device.
//! * An interrupted stage leaves an unverified candidate on the media that the
//!   boot policy refuses to select, and the slot that was running keeps
//!   running.
//! * A rollback is part of the same commit as the boot attempt it spent, so
//!   the device can never record "this slot is abandoned" while still
//!   pointing at it.

use alloc::vec::Vec;

use crate::policy::{
    account_boot_failure, accepts_version, mark_corrupted, mark_unverified, promote_to_testing,
    promote_to_valid, select_boot_slot, stage_candidate,
};
use crate::sha256::{constant_time_eq_32, sha256};
use crate::state::{OtaError, OtaState, Slot, SlotStatus};
use crate::store::OtaStore;

/// The A/B update engine over a durable store.
pub struct OtaEngine<S: OtaStore> {
    store: S,
    state: OtaState,
    fresh: bool,
}

impl<S: OtaStore> OtaEngine<S> {
    /// Opens the engine over `store`.
    ///
    /// The store is read here and nothing is written: opening must be safe to
    /// repeat on every boot, and a device that cannot be written must not be
    /// confused with a device that has nothing committed. Use
    /// [`OtaEngine::reconcile`] to perform the boot-time repair pass.
    pub fn open(store: S, current_version: u32) -> Result<OtaEngine<S>, OtaError> {
        let mut engine = OtaEngine {
            store,
            state: OtaState::initial(current_version),
            fresh: true,
        };
        if let Some(state) = engine.store.load()? {
            engine.state = state;
            engine.fresh = false;
        }
        Ok(engine)
    }

    /// The state the engine currently holds in memory.
    pub fn state(&self) -> &OtaState {
        &self.state
    }

    /// The store the engine writes through.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The store the engine writes through, mutably.
    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// Whether the device has a committed state yet.
    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    /// The slot the device should boot, if any.
    pub fn boot_slot(&self) -> Option<Slot> {
        select_boot_slot(&self.state)
    }

    /// The slot an update would be staged into.
    pub fn inactive_slot(&self) -> Slot {
        self.state.active_slot.alternate()
    }

    /// Whether a candidate version may be installed.
    pub fn accepts_version(&self, version: u32) -> bool {
        accepts_version(&self.state, version)
    }

    /// Re-reads the durable state and adopts it.
    ///
    /// WHY THIS EXISTS: after a failed write the engine must not keep a state
    /// that the media does not hold. The device is the authority; re-reading
    /// it turns an uncertain outcome into a known one. If the re-read also
    /// fails, the in-memory state is left untouched and the caller is told the
    /// device could not be read.
    pub fn resync(&mut self) -> Result<(), OtaError> {
        match self.store.load()? {
            Some(state) => {
                self.state = state;
                self.fresh = false;
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Writes `candidate` to the device and adopts it only on success.
    fn commit(&mut self, candidate: OtaState) -> Result<(), OtaError> {
        match self.store.save(&candidate) {
            Ok(()) => {
                self.state = candidate;
                Ok(())
            }
            Err(err) => {
                // The write did not succeed. Adopt whatever the device
                // actually holds so the in-memory state never claims more
                // than the media does, and report the failure.
                let _ = self.resync();
                Err(err)
            }
        }
    }

    /// Records that a slot's content failed verification.
    fn record_corruption(&mut self, slot: Slot) -> Result<(), OtaError> {
        let candidate = mark_corrupted(&self.state, slot);
        self.commit(candidate)
    }

    /// Verifies a slot's stored bytes against the digest the metadata names.
    ///
    /// `Ok(true)` means the bytes are present and match. `Ok(false)` means they
    /// are present and do NOT match, or are missing entirely. A slot whose
    /// metadata records no stored image (the factory image, which lives in the
    /// boot area and not in the OTA region) has nothing to verify and is
    /// reported as matching: the metadata record's own digest protects the
    /// recorded length, so this cannot be used to skip a check.
    fn verify_slot(&mut self, slot: Slot) -> Result<bool, OtaError> {
        let expected = self.state.slot(slot).content_digest;
        if self.state.slot(slot).content_len == 0 {
            return Ok(true);
        }
        match self.store.read_slot(slot) {
            Ok(Some(bytes)) => Ok(constant_time_eq_32(&sha256(&bytes), &expected)),
            Ok(None) => Ok(false),
            // The bytes are there and they are not what the header says. That
            // is a mismatch to be acted on, not a device failure.
            Err(OtaError::SlotDigestMismatch) => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Publishes an explicit state.
    ///
    /// The candidate is written to the device first and adopted in memory only
    /// after the write succeeded, exactly like every other transition here.
    pub fn commit_state(&mut self, candidate: OtaState) -> Result<(), OtaError> {
        self.commit(candidate)
    }

    /// The boot-time repair pass.
    ///
    /// It does three things, in this order:
    ///
    /// 1. On a device that has never been updated, it records the factory
    ///    baseline, so the device has a durable state from the first boot.
    /// 2. It re-verifies a slot that is on a trial boot. A trial slot whose
    ///    bytes no longer match is marked corrupted, so it is never booted.
    /// 3. It makes sure the active slot may be booted at all. If it may not,
    ///    and the other slot may, the pointer moves to the other slot and the
    ///    move is committed — a durable rollback, not an in-memory one.
    pub fn reconcile(&mut self) -> Result<(), OtaError> {
        if self.fresh {
            let candidate = self.state;
            self.commit(candidate)?;
            self.fresh = false;
            return Ok(());
        }

        let active = self.state.active_slot;
        if self.state.slot(active).status == SlotStatus::Testing {
            match self.verify_slot(active) {
                Ok(true) => {}
                Ok(false) => self.record_corruption(active)?,
                Err(err) => return Err(err),
            }
        }

        if let Some(boot) = select_boot_slot(&self.state) {
            if boot != self.state.active_slot {
                let mut candidate = self.state;
                candidate.active_slot = boot;
                self.commit(candidate)?;
            }
        }
        Ok(())
    }

    /// Writes an image into the inactive slot and selects it for a trial boot.
    ///
    /// THE ORDER OF THE STEPS, and why each one is where it is:
    ///
    /// 1. The bytes are written to the inactive slot and flushed.
    /// 2. The bytes are read back and re-hashed. A write that did not land is
    ///    caught here, before any metadata names the slot.
    /// 3. The candidate is committed with the status `Unverified`. The durable
    ///    record now knows the candidate's version and digest, and the boot
    ///    policy still refuses to select it.
    /// 4. The bytes are read back and re-hashed a second time, immediately
    ///    before the candidate is selected.
    /// 5. The candidate is committed with the status `Testing` and the active
    ///    pointer moves to it.
    ///
    /// A crash between steps 3 and 5 leaves an unverified candidate on the
    /// media and the previously active slot still selected: the device keeps
    /// booting what it was booting, and the candidate is not booted.
    pub fn stage_image(
        &mut self,
        version: u32,
        image: &[u8],
        expected_digest: &[u8; 32],
    ) -> Result<Slot, OtaError> {
        if !self.accepts_version(version) {
            return Err(OtaError::AntiRollback);
        }
        let target = self.inactive_slot();

        self.store.write_slot(target, image)?;

        let observed = match self.store.read_slot(target)? {
            Some(bytes) => sha256(&bytes),
            None => return Err(OtaError::SlotContentMissing),
        };
        if !constant_time_eq_32(&observed, expected_digest) {
            return Err(OtaError::SlotDigestMismatch);
        }

        let candidate = stage_candidate(
            &self.state,
            target,
            version,
            image.len() as u32,
            *expected_digest,
        );
        self.commit(candidate)?;

        let observed = match self.store.read_slot(target)? {
            Some(bytes) => sha256(&bytes),
            None => return Err(OtaError::SlotContentMissing),
        };
        if !constant_time_eq_32(&observed, expected_digest) {
            // The candidate stays durably `Unverified`. It is not selected,
            // and the boot policy will not select it.
            return Err(OtaError::SlotDigestMismatch);
        }

        let testing = promote_to_testing(&self.state, target);
        self.commit(testing)?;
        Ok(target)
    }

    /// Records that the active slot's boot succeeded.
    ///
    /// THE PROMOTION IS A THREE-PART OPERATION, and the order is the whole
    /// point of this method:
    ///
    /// 1. The slot's bytes are read back from the device and re-hashed
    ///    against the digest the metadata recorded when the slot was staged.
    ///    A slot whose bytes do not match is never promoted.
    /// 2. The `Valid` state is written to the device.
    /// 3. Only after that write has succeeded and been read back does the
    ///    in-memory state become `Valid`.
    ///
    /// If step 2 fails, the in-memory state is re-read from the device, so
    /// the engine never claims a promotion the media does not hold, and the
    /// failure is returned. The engine then tries to record the slot as
    /// `Unverified`: that is the fail-safe direction, because it can only stop
    /// the slot from being booted, never start it.
    pub fn commit_successful_boot(&mut self) -> Result<(), OtaError> {
        let active = self.state.active_slot;
        match self.state.slot(active).status {
            SlotStatus::Valid => return Ok(()),
            SlotStatus::Testing => {}
            _ => return Err(OtaError::SlotNotBootable),
        }

        if !self.store.holds_slot_content() {
            return Err(OtaError::SlotContentUnsupported);
        }

        let expected = self.state.slot(active).content_digest;
        let observed = match self.store.read_slot(active) {
            Ok(Some(bytes)) => sha256(&bytes),
            Ok(None) => return Err(OtaError::SlotContentMissing),
            Err(OtaError::SlotDigestMismatch) => {
                self.record_corruption(active)?;
                return Err(OtaError::SlotDigestMismatch);
            }
            Err(err) => return Err(err),
        };
        if !constant_time_eq_32(&observed, &expected) {
            self.record_corruption(active)?;
            return Err(OtaError::SlotDigestMismatch);
        }

        let candidate = promote_to_valid(&self.state, active);
        match self.store.save(&candidate) {
            Ok(()) => {
                self.state = candidate;
                Ok(())
            }
            Err(_) => {
                // The promotion did not reach the device. Re-read what the
                // device holds, then record the slot as unverified so it
                // cannot be booted as a confirmed image. The store's own
                // reason is deliberately not what the caller is told: the
                // contract of this call is that the *commit* failed, and
                // `PersistenceFailed` is the one answer that stays true
                // whatever the lower layer said.
                let _ = self.resync();
                let poison = mark_unverified(&self.state, active);
                match self.store.save(&poison) {
                    Ok(()) => {
                        self.state = poison;
                        Err(OtaError::PersistenceFailed)
                    }
                    Err(_) => {
                        let _ = self.resync();
                        Err(OtaError::PersistenceFailed)
                    }
                }
            }
        }
    }

    /// Records that the active slot failed to boot, and rolls back when the
    /// boot attempts are exhausted.
    ///
    /// The persistence error is returned, never swallowed: a counter that was
    /// never written must not look like a counter that was. The rollback is
    /// part of the same commit as the spent attempt, so the device can never
    /// hold "this slot is abandoned" while its pointer still names it.
    pub fn handle_boot_failure(&mut self) -> Result<Slot, OtaError> {
        let candidate = account_boot_failure(&self.state);
        match self.store.save(&candidate) {
            Ok(()) => {
                self.state = candidate;
                Ok(candidate.active_slot)
            }
            Err(err) => {
                let _ = self.resync();
                Err(err)
            }
        }
    }

    /// The bytes the given slot holds, read back and verified.
    pub fn read_slot(&mut self, slot: Slot) -> Result<Option<Vec<u8>>, OtaError> {
        self.store.read_slot(slot)
    }
}
