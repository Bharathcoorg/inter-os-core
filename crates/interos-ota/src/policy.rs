// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The pure decisions of the A/B update engine.
//!
//! WHY THESE ARE FUNCTIONS AND NOT METHODS ON A STRUCT: every one of them is a
//! pure mapping from one durable state to the next one. Keeping them free of
//! device access is what lets a host test prove the *policy* — when a slot may
//! be promoted, when a boot attempt is spent, when a rollback happens —
//! without a disk, an emulator or a target-specific register.
//!
//! THE INVARIANT EVERY FUNCTION HERE upholds: none of them mutates anything.
//! They return a candidate state. The caller writes that candidate to the
//! device first and adopts it in memory only after the write succeeded. That
//! ordering is the whole point: an in-memory state that claims something the
//! disk does not is how a failed update turns into a booted unverified image.

use crate::state::{OtaState, Slot, SlotStatus};

/// Boot attempts a freshly staged slot starts with.
pub const DEFAULT_BOOT_TRIES: u8 = 3;

/// The slot the device should boot, if any.
///
/// WHY THE ACTIVE SLOT IS CHECKED FIRST: the active slot is the one the last
/// commit selected. The fallback to the other slot exists for the recovery
/// case — the active slot was found corrupt or unverified on load, and the
/// other slot holds the last image that did boot.
///
/// WHY `Unverified` AND `Corrupted` ARE NEVER RETURNED: an unverified slot is
/// what an interrupted update leaves behind, and a corrupted slot failed its
/// own integrity check. Booting either is exactly the failure this subsystem
/// exists to prevent.
pub fn select_boot_slot(state: &OtaState) -> Option<Slot> {
    let active = state.active_slot;
    if state.slot(active).status.is_bootable() {
        return Some(active);
    }
    let alternate = active.alternate();
    if state.slot(alternate).status.is_bootable() {
        return Some(alternate);
    }
    None
}

/// Whether a candidate version may be installed.
///
/// WHY THE MAXIMUM OVER BOTH SLOTS: anti-rollback has to compare against the
/// newest version the device has ever accepted, not against the version of the
/// slot that happens to be active. After a rollback the active slot is the
/// older one, and comparing against it would re-open the downgrade the
/// rollback was performed to stop.
pub fn accepts_version(state: &OtaState, version: u32) -> bool {
    version > state.highest_version()
}

/// The state after one failed boot of the active slot.
///
/// THE RULES, in order:
/// * One boot attempt is spent. The counter saturates at zero, so a
///   pathological caller cannot underflow it.
/// * When the counter reaches zero the slot is abandoned: it is marked
///   `Corrupted` and the active pointer moves to the other slot. The rollback
///   is part of this same candidate state, so it becomes durable in the same
///   commit as the spent attempt — there is no window in which the counter
///   says "abandoned" but the pointer still names the bad slot.
pub fn account_boot_failure(state: &OtaState) -> OtaState {
    let mut next = *state;
    let active = next.active_slot;
    {
        let meta = next.slot_mut(active);
        meta.boot_tries_left = meta.boot_tries_left.saturating_sub(1);
        if meta.boot_tries_left == 0 {
            meta.status = SlotStatus::Corrupted;
        }
    }
    if next.slot(active).status == SlotStatus::Corrupted {
        next.active_slot = active.alternate();
    }
    next
}

/// The state after a candidate slot's image has been written and verified on
/// the device but before it is selected for a trial boot.
///
/// WHY THE CANDIDATE IS `Unverified` HERE: this is the durable statement "an
/// image is on the media, and this is the digest it must have". It is written
/// BEFORE the slot is selected, so a crash between the two commits leaves an
/// unverified candidate on the media that the boot policy refuses to touch.
pub fn stage_candidate(
    state: &OtaState,
    slot: Slot,
    version: u32,
    content_len: u32,
    content_digest: [u8; 32],
) -> OtaState {
    let mut next = *state;
    {
        let meta = next.slot_mut(slot);
        meta.version = version;
        meta.status = SlotStatus::Unverified;
        meta.boot_tries_left = 0;
        meta.content_len = content_len;
        meta.content_digest = content_digest;
    }
    next
}

/// The state after a candidate's bytes were re-read from the device and their
/// digest matched the expected one: the slot is now on a trial boot.
pub fn promote_to_testing(state: &OtaState, slot: Slot) -> OtaState {
    let mut next = *state;
    {
        let meta = next.slot_mut(slot);
        meta.status = SlotStatus::Testing;
        meta.boot_tries_left = DEFAULT_BOOT_TRIES;
    }
    next.active_slot = slot;
    next
}

/// The state after a slot's bytes failed verification.
pub fn mark_corrupted(state: &OtaState, slot: Slot) -> OtaState {
    let mut next = *state;
    let meta = next.slot_mut(slot);
    meta.status = SlotStatus::Corrupted;
    meta.boot_tries_left = 0;
    next
}

/// The state after a slot must not be booted any more, but the reason is not
/// that its content is known bad.
///
/// WHY THIS EXISTS: when the promotion of a verified slot cannot be recorded
/// on the device, the device cannot say "this image booted successfully". The
/// honest durable statement is then "this image is not confirmed", and the
/// boot policy refuses to boot it. Writing this state is the fail-safe
/// direction: it can only ever stop a boot, never start one.
pub fn mark_unverified(state: &OtaState, slot: Slot) -> OtaState {
    let mut next = *state;
    let meta = next.slot_mut(slot);
    meta.status = SlotStatus::Unverified;
    meta.boot_tries_left = 0;
    next
}

/// The state after a slot's boot succeeded: the slot is confirmed good.
pub fn promote_to_valid(state: &OtaState, slot: Slot) -> OtaState {
    let mut next = *state;
    {
        let meta = next.slot_mut(slot);
        meta.status = SlotStatus::Valid;
        meta.boot_tries_left = DEFAULT_BOOT_TRIES;
    }
    next
}
