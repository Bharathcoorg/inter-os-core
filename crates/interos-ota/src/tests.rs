// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Host-runnable tests for the A/B OTA state machine and its durable store.
//!
//! EVERY TEST HERE RUNS ON AN x86_64 HOST with no emulator and no target
//! toolchain, because the whole point of splitting the architecture-neutral
//! logic into its own crate is that the failure cases can be tested cheaply
//! and exhaustively. The fault-injecting fake block device in `fake_device.rs`
//! is the instrument: it can fail a write at any sector, land a sector
//! half-written, fail a flush, refuse a read, shrink the device or corrupt
//! the media under the store's feet.

use std::prelude::v1::*;

use crate::engine::OtaEngine;
use crate::fake_device::FakeDevice;
use crate::policy::{account_boot_failure, select_boot_slot};
use crate::sha256::sha256;
use crate::state::{MetadataRecord, OtaError, OtaState, Slot, SlotStatus};
use crate::store::{BlockDeviceOtaStore, Layout, OtaStore};

const DEVICE_SECTORS: u32 = 4096;
const CURRENT_VERSION: u32 = 540;

fn layout() -> Layout {
    Layout::default_ota_region()
}

fn fresh_device() -> FakeDevice {
    FakeDevice::new(DEVICE_SECTORS)
}

fn engine(device: FakeDevice) -> OtaEngine<BlockDeviceOtaStore<FakeDevice>> {
    let mut engine = OtaEngine::open(BlockDeviceOtaStore::new(device, layout()), CURRENT_VERSION)
        .expect("the engine must open");
    engine.reconcile().expect("the first reconcile must succeed");
    engine
}

/// A payload and the digest the metadata will name.
fn payload(version: u32) -> (Vec<u8>, [u8; 32]) {
    let bytes = format!("inter-os aarch64 ota payload version {version}").into_bytes();
    let digest = sha256(&bytes);
    (bytes, digest)
}

/// Stages a candidate and returns the slot it landed in.
fn stage(engine: &mut OtaEngine<BlockDeviceOtaStore<FakeDevice>>, version: u32) -> Slot {
    let (bytes, digest) = payload(version);
    engine
        .stage_image(version, &bytes, &digest)
        .expect("the stage must succeed")
}

// ---------------------------------------------------------------------------
// The hash the whole subsystem trusts.
// ---------------------------------------------------------------------------

/// The SHA-256 this crate trusts is the standard one. A state machine that
/// hashes differently from the kernel would compare a payload against a
/// digest the kernel never produced, and every promotion would fail.
#[test]
fn sha256_matches_the_fips_test_vectors() {
    assert_eq!(
        sha256(b""),
        [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ]
    );
    assert_eq!(
        sha256(b"abc"),
        [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ]
    );
}

// ---------------------------------------------------------------------------
// The metadata record.
// ---------------------------------------------------------------------------

/// A record survives the trip through the media and comes back byte for byte.
#[test]
fn a_metadata_record_round_trips() {
    let state = OtaState::initial(CURRENT_VERSION);
    let record = MetadataRecord {
        generation: 7,
        state,
    };
    let bytes = record.encode();
    assert_eq!(MetadataRecord::decode(&bytes).expect("must decode"), record);
}

/// A single flipped byte in a record makes it not a record at all. This is
/// what makes a torn write harmless rather than merely unlikely.
#[test]
fn a_tampered_metadata_record_is_refused() {
    let record = MetadataRecord {
        generation: 7,
        state: OtaState::initial(CURRENT_VERSION),
    };
    let mut bytes = record.encode();
    bytes[70] ^= 0x01;
    assert_eq!(
        MetadataRecord::decode(&bytes).err(),
        Some(OtaError::MetadataCorrupt),
        "a record whose digest does not verify must not be believed",
    );
}

/// A record that was never written is not an error, it is an absence.
#[test]
fn an_empty_sector_holds_no_record() {
    assert_eq!(
        MetadataRecord::decode(&[0u8; 512]).err(),
        Some(OtaError::MetadataCorrupt)
    );
}

// ---------------------------------------------------------------------------
// The store.
// ---------------------------------------------------------------------------

/// A fresh device has no committed state, and the engine's baseline is the
/// factory image.
#[test]
fn a_fresh_device_has_no_committed_state() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    assert_eq!(store.load().expect("the load must succeed"), None);
}

/// A committed state is on the device and comes back after a power cycle.
#[test]
fn a_committed_state_survives_a_power_cycle() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    let state = OtaState::initial(CURRENT_VERSION);
    store.save(&state).expect("the commit must succeed");

    // The power cycle: the media is kept, the engine is not.
    let mut reopened = BlockDeviceOtaStore::new(store.device().clone_media(), layout());
    assert_eq!(reopened.load().expect("the load must succeed"), Some(state));
}

/// The commit is confirmed by reading the record back off the device, so the
/// generation the device holds is the generation that was committed.
#[test]
fn a_commit_is_confirmed_by_a_read_back() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit");
    assert_eq!(store.next_generation(), 1);
}

/// The second commit goes to the other metadata copy, so the newest record is
/// never the one being overwritten.
#[test]
fn a_commit_writes_to_the_copy_that_is_not_the_newest() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit 1");
    assert_eq!(store.device().write_log.last().copied(), Some(layout().meta_a_lba));
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit 2");
    assert_eq!(store.device().write_log.last().copied(), Some(layout().meta_b_lba));
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit 3");
    assert_eq!(store.device().write_log.last().copied(), Some(layout().meta_a_lba));
}

/// A torn newest copy is discarded and the older one is selected. This is the
/// "a crash between the two metadata copies leaves the previous generation
/// selected" invariant, seen from the reader's side.
#[test]
fn a_corrupted_newest_metadata_copy_falls_back_to_the_older_one() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit 1");
    let mut second = OtaState::initial(CURRENT_VERSION);
    second.slot_b.version = 550;
    store.save(&second).expect("commit 2");

    // The newest copy (B) is garbled.
    let mut device = store.device().clone_media();
    device.corrupt(layout().meta_b_lba, 30, 0xFF);

    let mut reopened = BlockDeviceOtaStore::new(device, layout());
    let recovered = reopened.load().expect("the load must succeed").expect("a state");
    assert_eq!(
        recovered.slot_b.version, 0,
        "the corrupted copy must not be selected"
    );
}

/// A commit that never lands leaves the previous generation selected.
#[test]
fn an_interrupted_commit_leaves_the_previous_generation_selected() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit 1");
    let mut second = OtaState::initial(CURRENT_VERSION);
    second.slot_b.version = 550;

    // The write of the second record fails.
    let mut failing = store.device().clone_media();
    failing.fail_next_write = true;
    let mut failing_store = BlockDeviceOtaStore::new(failing, layout());
    assert!(failing_store.save(&second).is_err());

    // The device still holds the first generation.
    let mut reopened = BlockDeviceOtaStore::new(store.device().clone_media(), layout());
    let recovered = reopened.load().expect("the load must succeed").expect("a state");
    assert_eq!(recovered.slot_b.version, 0);
    assert_eq!(reopened.next_generation(), 1);
}

/// A sector that lands half-written is not a record: its digest does not
/// verify, so the reader falls back to the other copy.
#[test]
fn a_half_written_metadata_copy_is_discarded() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    store.save(&OtaState::initial(CURRENT_VERSION)).expect("commit 1");
    let mut second = OtaState::initial(CURRENT_VERSION);
    second.slot_b.version = 550;

    let mut failing = store.device().clone_media();
    failing.partial_write = Some((layout().meta_b_lba, 40));
    let mut failing_store = BlockDeviceOtaStore::new(failing, layout());
    assert!(failing_store.save(&second).is_err());

    let mut reopened = BlockDeviceOtaStore::new(store.device().clone_media(), layout());
    let recovered = reopened.load().expect("the load must succeed").expect("a state");
    assert_eq!(recovered.slot_b.version, 0);
}

/// Two copies at the same generation with different contents cannot be
/// resolved, and the store refuses rather than guessing which image to boot.
#[test]
fn two_copies_at_the_same_generation_with_different_contents_are_refused() {
    let device = fresh_device();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    let mut first = OtaState::initial(CURRENT_VERSION);
    first.slot_b.version = 550;
    store.save(&first).expect("commit");

    // A foreign writer publishes a different record at the same generation.
    let mut other = first;
    other.slot_b.version = 551;
    let bytes = MetadataRecord {
        generation: 1,
        state: other,
    }
    .encode();
    let mut device = store.device().clone_media();
    device.poke(layout().meta_b_lba, 0, &bytes);

    let mut reopened = BlockDeviceOtaStore::new(device, layout());
    assert_eq!(
        reopened.load().err(),
        Some(OtaError::MetadataAmbiguous),
        "an ambiguous generation must be refused, not guessed",
    );
}

/// The store writes only inside its own region. A user-data sector next to the
/// OTA region is never touched.
#[test]
fn the_store_never_writes_outside_its_region() {
    let mut device = fresh_device();
    device.poke(100, 0, b"USER DATA");
    let before = device.sector(100).to_vec();

    let mut engine = engine(device);
    let (bytes, digest) = payload(550);
    engine.stage_image(550, &bytes, &digest).expect("stage");
    engine.commit_successful_boot().expect("promote");
    engine.handle_boot_failure().expect("failure");

    let device = engine.store().device();
    assert_eq!(device.sector(100), before.as_slice(), "user data must survive");
    let region_start = layout().meta_a_lba;
    let region_end = layout().total_sectors();
    for lba in &device.write_log {
        assert!(
            *lba >= region_start && *lba < region_end,
            "a write to LBA {lba} is outside the OTA region",
        );
    }
}

/// A device that still has to boot something is refused: the OTA region must
/// not be written over a boot area.
#[test]
fn a_bootable_device_is_refused() {
    let mut device = fresh_device();
    device.install_boot_signature();
    let mut store = BlockDeviceOtaStore::new(device, layout());
    assert_eq!(
        store.save(&OtaState::initial(CURRENT_VERSION)).err(),
        Some(OtaError::BootDeviceProtected),
    );
}

/// A device smaller than the OTA region is refused rather than written past
/// its end.
#[test]
fn a_device_too_small_for_the_region_is_refused() {
    let device = FakeDevice::new(16);
    let mut store = BlockDeviceOtaStore::new(device, layout());
    assert_eq!(store.load().err(), Some(OtaError::LayoutOutOfRange));
}

// ---------------------------------------------------------------------------
// Slot content.
// ---------------------------------------------------------------------------

/// A staged slot's bytes are read back from the device and re-hashed.
#[test]
fn a_staged_slot_is_read_back_and_verified() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let slot = engine.stage_image(550, &bytes, &digest).expect("stage");
    let read_back = engine.read_slot(slot).expect("read").expect("content");
    assert_eq!(read_back, bytes);
    assert_eq!(sha256(&read_back), digest);
}

/// A payload byte that changed on the media is detected when the slot is read
/// back, and the slot is never promoted.
#[test]
fn a_corrupted_payload_is_detected_and_never_promoted() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let slot = engine.stage_image(550, &bytes, &digest).expect("stage");

    // The media is altered behind the engine's back.
    let payload_lba = layout().slot_payload_base(slot);
    engine.store_mut().device_mut().corrupt(payload_lba, 3, 0xFF);

    assert_eq!(
        engine.read_slot(slot).err(),
        Some(OtaError::SlotDigestMismatch),
        "a changed payload must not read back as the staged image",
    );
    assert_eq!(
        engine.commit_successful_boot().err(),
        Some(OtaError::SlotDigestMismatch),
        "a slot whose bytes changed must never be promoted",
    );
}

/// A slot whose header is gone holds no image.
#[test]
fn a_slot_without_a_header_holds_no_image() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let slot = engine.stage_image(550, &bytes, &digest).expect("stage");
    engine
        .store_mut()
        .device_mut()
        .poke(layout().slot_base(slot), 0, &[0u8; 8]);
    assert_eq!(engine.read_slot(slot).expect("read"), None);
}

/// A slot whose header was garbled is corrupt, not missing.
#[test]
fn a_slot_with_a_corrupt_header_is_reported_as_corrupt() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let slot = engine.stage_image(550, &bytes, &digest).expect("stage");
    engine
        .store_mut()
        .device_mut()
        .corrupt(layout().slot_base(slot), 30, 0x5A);
    assert_eq!(
        engine.read_slot(slot).err(),
        Some(OtaError::SlotHeaderCorrupt)
    );
}

/// An image larger than the slot cannot be written, and the attempt leaves
/// the metadata alone.
#[test]
fn an_image_too_large_for_the_slot_is_refused() {
    let mut engine = engine(fresh_device());
    let oversized = vec![0x41u8; layout().slot_capacity_bytes() + 1];
    let digest = sha256(&oversized);
    assert_eq!(
        engine.stage_image(550, &oversized, &digest).err(),
        Some(OtaError::SlotContentTooLarge),
    );
    assert_eq!(engine.state().active_slot, Slot::SlotA);
}

// ---------------------------------------------------------------------------
// Staging.
// ---------------------------------------------------------------------------

/// A staged candidate is on trial, and the device now boots it.
#[test]
fn a_staged_candidate_is_on_trial_and_is_selected() {
    let mut engine = engine(fresh_device());
    let slot = stage(&mut engine, 550);
    assert_eq!(slot, Slot::SlotB);
    assert_eq!(engine.state().active_slot, Slot::SlotB);
    assert_eq!(engine.state().slot_b.status, SlotStatus::Testing);
    assert_eq!(engine.state().slot_b.version, 550);
    assert_eq!(engine.state().slot_b.boot_tries_left, 3);
    assert_eq!(engine.boot_slot(), Some(Slot::SlotB));
}

/// The stage survives a power cycle with its version, its digest and its
/// boot-attempt count intact.
#[test]
fn a_staged_candidate_survives_a_power_cycle() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    engine.stage_image(550, &bytes, &digest).expect("stage");

    let device = engine.store().device().clone_media();
    let mut reopened = engine_over(device);
    assert_eq!(reopened.state().active_slot, Slot::SlotB);
    assert_eq!(reopened.state().slot_b.version, 550);
    assert_eq!(reopened.state().slot_b.status, SlotStatus::Testing);
    assert_eq!(reopened.state().slot_b.boot_tries_left, 3);
    assert_eq!(reopened.state().slot_b.content_digest, digest);
}

/// A version that is not newer than every version the device has accepted is
/// refused.
#[test]
fn anti_rollback_refuses_an_older_version() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    engine.stage_image(550, &bytes, &digest).expect("stage");

    let (older, older_digest) = payload(530);
    assert_eq!(
        engine.stage_image(530, &older, &older_digest).err(),
        Some(OtaError::AntiRollback),
    );
}

/// The anti-rollback floor is the newest version the device ever accepted, not
/// the version of the slot that happens to be active. After a rollback to the
/// older slot the floor stays where it was.
#[test]
fn the_anti_rollback_floor_survives_a_rollback() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    engine.stage_image(550, &bytes, &digest).expect("stage");
    for _ in 0..3 {
        engine.handle_boot_failure().expect("failure");
    }
    assert_eq!(engine.state().active_slot, Slot::SlotA);
    let (older, older_digest) = payload(545);
    assert_eq!(
        engine.stage_image(545, &older, &older_digest).err(),
        Some(OtaError::AntiRollback),
        "a rollback must not re-open the downgrade it was performed to stop",
    );
}

/// A stage whose metadata commit fails leaves the candidate unverified and the
/// previously active slot still selected.
#[test]
fn an_interrupted_stage_leaves_the_candidate_unverified_and_not_bootable() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);

    // Fail the commit that would move the candidate from `Unverified` to
    // `Testing`. The first commit (the `Unverified` record) succeeds.
    let mut engine = engine;
    let target = layout().meta_a_lba;
    engine.store_mut().device_mut().fail_write_at_lba = Some(target);
    assert!(engine.stage_image(550, &bytes, &digest).is_err());
    engine.store_mut().device_mut().fail_write_at_lba = None;

    assert_eq!(
        engine.state().active_slot,
        Slot::SlotA,
        "the candidate must not have been selected",
    );
    assert_eq!(
        engine.state().slot_b.status,
        SlotStatus::Unverified,
        "the candidate must be durably unverified",
    );
    assert_eq!(
        engine.boot_slot(),
        Some(Slot::SlotA),
        "an unverified candidate must never be booted",
    );
}

/// A stage whose image write fails leaves the metadata exactly as it was.
#[test]
fn a_failed_slot_write_leaves_the_state_untouched() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    engine.store_mut().device_mut().fail_next_write = true;
    assert!(engine.stage_image(550, &bytes, &digest).is_err());
    assert_eq!(engine.state().active_slot, Slot::SlotA);
    assert_eq!(engine.state().slot_b.status, SlotStatus::Corrupted);
    assert_eq!(engine.boot_slot(), Some(Slot::SlotA));
}

/// A half-written image is caught by the read-back: the stage fails and the
/// candidate is never selected.
#[test]
fn a_partially_written_image_is_detected() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let payload_lba = layout().slot_payload_base(Slot::SlotB);
    engine.store_mut().device_mut().partial_write = Some((payload_lba, 8));
    assert!(engine.stage_image(550, &bytes, &digest).is_err());
    assert_eq!(engine.state().active_slot, Slot::SlotA);
    assert_eq!(engine.boot_slot(), Some(Slot::SlotA));
}

// ---------------------------------------------------------------------------
// Promotion.
// ---------------------------------------------------------------------------

/// A successful boot promotes the trial slot, and the promotion is durable.
#[test]
fn a_successful_boot_promotes_the_slot_durably() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);
    engine.commit_successful_boot().expect("promote");
    assert_eq!(engine.state().slot_b.status, SlotStatus::Valid);

    let device = engine.store().device().clone_media();
    let reopened = engine_over(device);
    assert_eq!(reopened.state().slot_b.status, SlotStatus::Valid);
    assert_eq!(reopened.state().active_slot, Slot::SlotB);
}

/// A promotion whose commit fails does not mark the slot valid, and the slot
/// is recorded as unverified so it cannot be booted.
#[test]
fn a_failed_promotion_commit_does_not_mark_the_slot_valid() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);

    // The promotion's record write fails. The fail-safe write that follows
    // it succeeds, so the slot ends up durably unverified.
    engine.store_mut().device_mut().fail_next_write = true;
    assert_eq!(
        engine.commit_successful_boot().err(),
        Some(OtaError::PersistenceFailed),
        "the failure must be reported, not swallowed",
    );
    assert_eq!(
        engine.state().slot_b.status,
        SlotStatus::Unverified,
        "a promotion that was not committed must leave the slot unverified",
    );
    assert_ne!(
        engine.boot_slot(),
        Some(Slot::SlotB),
        "a slot whose promotion was not committed must not be booted",
    );
}

/// When the promotion commit fails and the fail-safe write fails too, the
/// in-memory state is re-read from the device, so the two agree.
#[test]
fn a_failed_promotion_commit_leaves_memory_and_media_agreeing() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);
    let before = *engine.state();

    engine.store_mut().device_mut().fail_write_at_lba = Some(layout().meta_b_lba);
    assert!(engine.commit_successful_boot().is_err());
    engine.store_mut().device_mut().fail_write_at_lba = None;

    let device = engine.store().device().clone_media();
    let mut reopened = BlockDeviceOtaStore::new(device, layout());
    let durable = reopened.load().expect("load").expect("a state");
    assert_eq!(
        engine.state(),
        &durable,
        "the in-memory state must never claim more than the device holds",
    );
    assert_eq!(
        engine.state(),
        &before,
        "a promotion that was never committed must leave the state as it was",
    );
}

/// A promotion is refused when the slot's bytes no longer match what the
/// metadata recorded, and the slot is recorded as corrupted.
#[test]
fn a_promotion_of_a_changed_slot_is_refused_and_recorded() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let slot = engine.stage_image(550, &bytes, &digest).expect("stage");
    engine
        .store_mut()
        .device_mut()
        .corrupt(layout().slot_payload_base(slot), 1, 0x01);

    assert!(engine.commit_successful_boot().is_err());
    assert_eq!(
        engine.state().slot_b.status,
        SlotStatus::Corrupted,
        "a slot whose bytes changed must be recorded as corrupted",
    );
    assert_eq!(engine.boot_slot(), Some(Slot::SlotA));
}

// ---------------------------------------------------------------------------
// Boot attempts and rollback.
// ---------------------------------------------------------------------------

/// A failed boot spends an attempt, and the count survives a power cycle.
#[test]
fn a_failed_boot_spends_an_attempt_that_survives_a_power_cycle() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);
    assert_eq!(engine.handle_boot_failure().expect("failure"), Slot::SlotB);
    assert_eq!(engine.state().slot_b.boot_tries_left, 2);

    let device = engine.store().device().clone_media();
    let reopened = engine_over(device);
    assert_eq!(reopened.state().slot_b.boot_tries_left, 2);
    assert_eq!(reopened.state().active_slot, Slot::SlotB);
}

/// A boot-failure whose commit fails is reported, and the counter is not
/// decremented in a way the device does not know about.
#[test]
fn a_failed_boot_failure_commit_is_reported() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);
    let before = *engine.state();

    engine.store_mut().device_mut().fail_next_write = true;
    assert_eq!(
        engine.handle_boot_failure().err(),
        Some(OtaError::DeviceWriteFailed),
        "the persistence error must be propagated, never swallowed",
    );
    assert_eq!(
        engine.state(),
        &before,
        "a counter that was never written must not look written",
    );
}

/// The rollback is durable: after the attempts are exhausted the device
/// points at the old slot, and that is what the next boot sees.
#[test]
fn the_rollback_is_durable() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);
    for _ in 0..2 {
        assert_eq!(engine.handle_boot_failure().expect("failure"), Slot::SlotB);
    }
    assert_eq!(engine.handle_boot_failure().expect("failure"), Slot::SlotA);
    assert_eq!(engine.state().active_slot, Slot::SlotA);
    assert_eq!(engine.state().slot_b.status, SlotStatus::Corrupted);

    let device = engine.store().device().clone_media();
    let reopened = engine_over(device);
    assert_eq!(
        reopened.state().active_slot,
        Slot::SlotA,
        "the rollback must survive the power cycle",
    );
    assert_eq!(reopened.state().slot_b.status, SlotStatus::Corrupted);
    assert_eq!(reopened.boot_slot(), Some(Slot::SlotA));
}

/// A rollback commit that never lands leaves the previous generation selected,
/// so the device still knows which slot it was on.
#[test]
fn an_interrupted_rollback_leaves_the_previous_generation_selected() {
    let mut engine = engine(fresh_device());
    stage(&mut engine, 550);
    engine.handle_boot_failure().expect("failure 1");
    engine.handle_boot_failure().expect("failure 2");

    // The third failure would roll back. Its write fails.
    engine.store_mut().device_mut().fail_next_write = true;
    assert!(engine.handle_boot_failure().is_err());

    let device = engine.store().device().clone_media();
    let reopened = engine_over(device);
    assert_eq!(
        reopened.state().active_slot,
        Slot::SlotB,
        "an interrupted rollback must leave the previous generation selected",
    );
    assert_eq!(reopened.state().slot_b.boot_tries_left, 1);
}

// ---------------------------------------------------------------------------
// Recovery.
// ---------------------------------------------------------------------------

/// A trial slot whose bytes were damaged while the device was off is
/// abandoned at the next boot, and the other slot is selected and committed.
#[test]
fn recovery_from_a_corrupt_slot_selects_and_commits_the_other_slot() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    let slot = engine.stage_image(550, &bytes, &digest).expect("stage");

    let mut device = engine.store().device().clone_media();
    device.corrupt(layout().slot_payload_base(slot), 4, 0x7E);

    let mut reopened = engine_over(device);
    reopened.reconcile().expect("reconcile");
    assert_eq!(
        reopened.state().slot_b.status,
        SlotStatus::Corrupted,
        "a trial slot that no longer matches must be abandoned",
    );
    assert_eq!(reopened.state().active_slot, Slot::SlotA);
    assert_eq!(reopened.boot_slot(), Some(Slot::SlotA));

    // The repair is durable.
    let device = reopened.store().device().clone_media();
    let again = engine_over(device);
    assert_eq!(again.state().active_slot, Slot::SlotA);
    assert_eq!(again.state().slot_b.status, SlotStatus::Corrupted);
}

/// A device whose two slots are both unusable reports that it has no
/// bootable slot rather than booting one anyway.
#[test]
fn a_device_with_no_bootable_slot_reports_it() {
    let mut engine = engine(fresh_device());
    let (bytes, digest) = payload(550);
    engine.stage_image(550, &bytes, &digest).expect("stage");
    engine.handle_boot_failure().expect("failure");
    engine.handle_boot_failure().expect("failure");
    engine.handle_boot_failure().expect("failure");
    // Slot A is the factory image (no stored bytes to verify), so it is still
    // bootable; make it unusable too.
    let mut state = *engine.state();
    state.slot_a.status = SlotStatus::Corrupted;
    engine.commit_state(state);
    assert_eq!(engine.boot_slot(), None);
}

/// The pure boot policy refuses an unverified or corrupted slot even when it
/// is the active one.
#[test]
fn the_boot_policy_refuses_an_unverified_or_corrupted_active_slot() {
    let mut state = OtaState::initial(CURRENT_VERSION);
    state.active_slot = Slot::SlotB;
    state.slot_b.status = SlotStatus::Unverified;
    assert_eq!(select_boot_slot(&state), Some(Slot::SlotA));

    state.slot_a.status = SlotStatus::Corrupted;
    assert_eq!(select_boot_slot(&state), None);

    state.slot_b.status = SlotStatus::Testing;
    assert_eq!(select_boot_slot(&state), Some(Slot::SlotB));
}

/// The pure failure accounting decrements once and rolls back exactly when
/// the attempts are exhausted.
#[test]
fn the_failure_accounting_rolls_back_on_the_last_attempt() {
    let mut state = OtaState::initial(CURRENT_VERSION);
    state.active_slot = Slot::SlotB;
    state.slot_b.status = SlotStatus::Testing;
    state.slot_b.boot_tries_left = 2;

    let first = account_boot_failure(&state);
    assert_eq!(first.slot_b.boot_tries_left, 1);
    assert_eq!(first.active_slot, Slot::SlotB);

    let second = account_boot_failure(&first);
    assert_eq!(second.slot_b.boot_tries_left, 0);
    assert_eq!(second.slot_b.status, SlotStatus::Corrupted);
    assert_eq!(
        second.active_slot,
        Slot::SlotA,
        "the exhausted slot must be abandoned",
    );
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Opens an engine over a device and runs the boot-time repair pass.
fn engine_over(device: FakeDevice) -> OtaEngine<BlockDeviceOtaStore<FakeDevice>> {
    let mut engine = OtaEngine::open(BlockDeviceOtaStore::new(device, layout()), CURRENT_VERSION)
        .expect("the engine must open");
    engine.reconcile().expect("the reconcile must succeed");
    engine
}
