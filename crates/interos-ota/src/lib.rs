// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Architecture-neutral A/B OTA state machine and durable slot store.
//!
//! WHAT THIS CRATE IS FOR: the A/B update engine has two halves that used to be
//! welded together inside one architecture kernel. The half that decides *what
//! the durable state should be* — the metadata record format, the generation
//! counter, the rule for choosing between two metadata copies, the rule for
//! promoting a candidate slot, the rule for spending a boot attempt, the rule
//! for rolling back — is pure logic and belongs where a host can test it. The
//! half that *touches hardware* — the virtio-mmio transport, the eMMC
//! controller, the sector cache — belongs in the architecture kernel. This
//! crate is the first half; `kernel-aarch64/src/ota.rs` is the second.
//!
//! WHY IT IS `no_std` AND DEPENDENCY-FREE: the state machine is linked into a
//! microkernel that has no standard library, and the repository forbids
//! third-party crates in constrained cores. The crate therefore brings its own
//! SHA-256 (`sha256.rs`, copied from the repository's
//! `kernel-aarch64/kernel-core/src/crypto/sha256.rs`) and uses `alloc` only for
//! the one buffer that has to grow with the payload.
//!
//! THE DURABILITY BOUNDARY, stated once and precisely:
//!
//! * A slot's *content* is durable when the payload sectors and the slot header
//!   sector have been written and the device has completed its flush. The
//!   header is written last and carries the payload's SHA-256, so a torn
//!   payload is detectable: reading the slot back re-hashes the bytes.
//! * The *metadata* is durable when one of the two metadata copies contains a
//!   record whose own digest verifies and whose generation is the highest
//!   among the copies that verify. A commit writes the record to the copy
//!   that does *not* hold the newest generation, so a crash during the write
//!   leaves the previous generation intact and selected.
//! * Nothing else in this crate is durable. A successful `save` is additionally
//!   confirmed by reading the record back off the device, so `Ok` means "the
//!   bytes were read back from the device", not "the write call returned".
//!
//! WHAT THIS CRATE DOES NOT CLAIM: the metadata digest is an unkeyed SHA-256.
//! It detects torn writes, bit rot and casual corruption. It does not stop an
//! attacker who can write the whole disk from forging a record and its digest;
//! that needs a keyed MAC anchored in a secret the attacker cannot read (the
//! repository's journal solves this with an HMAC over a device seed). The A/B
//! layout is crash-consistent, not tamper-proof.

#![no_std]

extern crate alloc;

pub mod engine;
pub mod policy;
pub mod sha256;
pub mod state;
pub mod store;

#[cfg(test)]
extern crate std;
#[cfg(test)]
mod fake_device;
#[cfg(test)]
mod tests;

pub use engine::OtaEngine;
pub use policy::{
    account_boot_failure, accepts_version, mark_corrupted, mark_unverified, promote_to_valid,
    select_boot_slot, stage_candidate,
};
pub use sha256::{constant_time_eq_32, sha256, Sha256, SHA256_DIGEST_LENGTH};
pub use state::{
    MetadataRecord, OtaError, OtaState, Slot, SlotMetadata, SlotStatus, META_MAGIC,
    META_RECORD_SIZE, META_VERSION, SECTOR_SIZE,
};
pub use store::{
    BlockDevice, BlockDeviceOtaStore, BlockError, Layout, OtaStore, SLOT_HEADER_MAGIC,
};
