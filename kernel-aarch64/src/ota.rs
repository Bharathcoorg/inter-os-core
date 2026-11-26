// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS — AArch64 Sovereign OTA Microkernel Updates & A/B Partition Rollback Engine
//!
//! Non-Negotiable Principle #1: User owns the machine.
//! Non-Negotiable Principle #17: Private state remains protected; public assertions use cryptographic proofs.
//!
//! Provides bare-metal A/B dual partition slot management, cryptographic update verification,
//! anti-rollback monotonic version enforcement, and watchdog boot counter rollback protection.

extern crate alloc;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::crypto::{constant_time_eq_32, sha256, verify_ed25519_signature};
use alloc::vec::Vec;

/// OTA Package Header Magic: "INTR_OTA"
pub const OTA_MAGIC: [u8; 8] = *b"INTR_OTA";

/// Architecture identifier for AArch64.
pub const ARCH_AARCH64: u16 = 0xAA64;

/// A/B Partition Slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PartitionSlot {
    SlotA,
    SlotB,
}

impl PartitionSlot {
    pub fn alternate(self) -> Self {
        match self {
            PartitionSlot::SlotA => PartitionSlot::SlotB,
            PartitionSlot::SlotB => PartitionSlot::SlotA,
        }
    }
}

/// Slot integrity and validation state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotStatus {
    Valid,
    Testing,
    Corrupted,
}

/// Metadata recorded for each partition slot.
#[derive(Clone, Debug)]
pub struct SlotMetadata {
    pub slot: PartitionSlot,
    pub version: u32,
    pub status: SlotStatus,
    pub boot_tries_left: u8,
    pub digest: [u8; 32],
}

/// Errors returned by the OTA management engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OtaError {
    CapabilityDenied,
    InvalidMagic,
    TargetArchitectureMismatch,
    TargetHardwareMismatch,
    AntiRollbackDowngrade,
    DigestMismatch,
    InvalidSignature,
    PayloadLengthMismatch,
    WatchdogExpired,
}

/// OTA update package header.
#[repr(C)]
#[derive(Clone, Debug)]
pub struct OtaPackageHeader {
    pub magic: [u8; 8],
    pub target_arch: u16,
    pub target_board: [u8; 16],
    pub version: u32,
    pub payload_len: u32,
    pub payload_sha256: [u8; 32],
    pub signature: [u8; 64],
}

impl OtaPackageHeader {
    /// Constructs the canonical metadata preimage covered by the publisher Ed25519 signature.
    pub fn compute_signature_preimage(&self) -> Vec<u8> {
        let mut preimage = Vec::new();
        preimage.extend_from_slice(&self.magic);
        preimage.extend_from_slice(&self.target_arch.to_be_bytes());
        preimage.extend_from_slice(&self.target_board);
        preimage.extend_from_slice(&self.version.to_be_bytes());
        preimage.extend_from_slice(&self.payload_len.to_be_bytes());
        preimage.extend_from_slice(&self.payload_sha256);
        preimage
    }
}

/// Durable snapshot of the A/B slot table written to non-volatile storage.
///
/// This is the on-media image of the manager's volatile metadata. It is a plain
/// `Clone` value (no heap) so a backing store can persist and reload it without
/// allocation in the boot path.
#[derive(Clone, Debug)]
pub struct PersistedOtaState {
    pub active_slot: PartitionSlot,
    pub slot_a: SlotMetadata,
    pub slot_b: SlotMetadata,
}

/// Persistent backend for OTA slot metadata and boot-tries counters.
///
/// The AArch64 kernel runs in QEMU without a real block device bound in this
/// crate's context, so the physical backend (eMMC/UFS A/B partition) is
/// board-gated: a board bring-up binds a concrete store (for example one backed
/// by `interos_kernel_core::storage::block::BlockDevice`). Until then the
/// default [`NoopStore`] keeps the engine RAM-only, and tests inject an
/// in-memory [`MemStore`](self) to prove durability across reconstruction.
pub trait OtaStore {
    /// Returns the last committed snapshot, or `Ok(None)` if nothing has been
    /// persisted yet (fresh device / first boot).
    fn load(&mut self) -> Result<Option<PersistedOtaState>, OtaError>;

    /// Commits a snapshot to durable media. Implementations must `flush`
    /// volatile caches before returning `Ok`.
    fn save(&mut self, state: &PersistedOtaState) -> Result<(), OtaError>;
}

/// Default store used by [`OtaManager::new`]: performs no persistence, keeping
/// the legacy RAM-only behavior for QEMU and callers that do not bind a device.
#[derive(Default)]
pub struct NoopStore;

impl OtaStore for NoopStore {
    fn load(&mut self) -> Result<Option<PersistedOtaState>, OtaError> {
        Ok(None)
    }

    fn save(&mut self, _state: &PersistedOtaState) -> Result<(), OtaError> {
        Ok(())
    }
}

/// Dual Partition A/B OTA & Autonomous Rollback Manager.
pub struct OtaManager<S: OtaStore = NoopStore> {
    pub active_slot: PartitionSlot,
    pub slot_a: SlotMetadata,
    pub slot_b: SlotMetadata,
    pub publisher_pubkey: [u8; 32],
    pub device_board_id: [u8; 16],
    store: S,
}

impl OtaManager<NoopStore> {
    /// Creates a new OTA manager with Slot A as initially active and valid.
    ///
    /// Uses the default [`NoopStore`], preserving the legacy RAM-only behavior
    /// for QEMU. Bind a durable backend with [`OtaManager::with_store`].
    pub fn new(current_version: u32, publisher_key: [u8; 32], board_id: [u8; 16]) -> Self {
        Self::from_parts(NoopStore, current_version, publisher_key, board_id)
    }
}

impl<S: OtaStore> OtaManager<S> {
    fn from_parts(
        store: S,
        current_version: u32,
        publisher_key: [u8; 32],
        board_id: [u8; 16],
    ) -> Self {
        Self {
            active_slot: PartitionSlot::SlotA,
            slot_a: SlotMetadata {
                slot: PartitionSlot::SlotA,
                version: current_version,
                status: SlotStatus::Valid,
                boot_tries_left: 3,
                digest: [0u8; 32],
            },
            slot_b: SlotMetadata {
                slot: PartitionSlot::SlotB,
                version: 0,
                status: SlotStatus::Corrupted,
                boot_tries_left: 0,
                digest: [0u8; 32],
            },
            publisher_pubkey: publisher_key,
            device_board_id: board_id,
            store,
        }
    }

    /// Creates a manager backed by `store`, restoring the persisted slot table
    /// when one exists. A fresh store (`Ok(None)`) yields the default layout.
    pub fn with_store(
        store: S,
        current_version: u32,
        publisher_key: [u8; 32],
        board_id: [u8; 16],
    ) -> Result<Self, OtaError> {
        let mut mgr = Self::from_parts(store, current_version, publisher_key, board_id);
        if let Some(state) = mgr.store.load()? {
            mgr.active_slot = state.active_slot;
            mgr.slot_a = state.slot_a;
            mgr.slot_b = state.slot_b;
        }
        Ok(mgr)
    }

    /// Writes the current slot table + boot-tries to the persistent backend.
    fn persist(&mut self) -> Result<(), OtaError> {
        let state = PersistedOtaState {
            active_slot: self.active_slot,
            slot_a: self.slot_a.clone(),
            slot_b: self.slot_b.clone(),
        };
        self.store.save(&state)
    }

    /// Returns the currently inactive slot targeted for staging updates.
    pub fn inactive_slot(&self) -> PartitionSlot {
        self.active_slot.alternate()
    }

    /// Returns a reference to the active slot metadata.
    pub fn active_slot_meta(&self) -> &SlotMetadata {
        match self.active_slot {
            PartitionSlot::SlotA => &self.slot_a,
            PartitionSlot::SlotB => &self.slot_b,
        }
    }

    /// Returns a mutable reference to the active slot metadata.
    pub fn active_slot_meta_mut(&mut self) -> &mut SlotMetadata {
        match self.active_slot {
            PartitionSlot::SlotA => &mut self.slot_a,
            PartitionSlot::SlotB => &mut self.slot_b,
        }
    }

    /// Returns a mutable reference to the inactive slot metadata.
    pub fn inactive_slot_meta_mut(&mut self) -> &mut SlotMetadata {
        match self.active_slot {
            PartitionSlot::SlotA => &mut self.slot_b,
            PartitionSlot::SlotB => &mut self.slot_a,
        }
    }

    /// Cryptographically verifies an OTA package and stages it into the inactive partition slot.
    pub fn verify_and_stage_update(
        &mut self,
        header: &OtaPackageHeader,
        payload: &[u8],
        cap_table: &CapabilityTable,
    ) -> Result<PartitionSlot, OtaError> {
        // Enforce Zero Ambient Authority: require CapabilityType::OtaUpdateManagement with Rights::WRITE
        cap_table
            .check(CapabilityType::OtaUpdateManagement, Rights::WRITE)
            .map_err(|_| OtaError::CapabilityDenied)?;

        // 1. Verify Magic
        if header.magic != OTA_MAGIC {
            return Err(OtaError::InvalidMagic);
        }

        // 2. Verify Architecture
        if header.target_arch != ARCH_AARCH64 {
            return Err(OtaError::TargetArchitectureMismatch);
        }

        // 3. Verify Hardware Target Device ID
        if header.target_board != self.device_board_id {
            return Err(OtaError::TargetHardwareMismatch);
        }

        // 4. Anti-Rollback Protection: version must be strictly greater than active valid version
        if header.version <= self.active_slot_meta().version {
            return Err(OtaError::AntiRollbackDowngrade);
        }

        // 5. Payload length validation
        if payload.len() != header.payload_len as usize {
            return Err(OtaError::PayloadLengthMismatch);
        }

        // 6. Cryptographic digest validation
        let computed_digest = sha256(payload);
        if !constant_time_eq_32(&computed_digest, &header.payload_sha256) {
            return Err(OtaError::DigestMismatch);
        }

        // 7. Cryptographic publisher signature validation
        let preimage = header.compute_signature_preimage();
        if !verify_ed25519_signature(&preimage, &self.publisher_pubkey, &header.signature) {
            return Err(OtaError::InvalidSignature);
        }

        // 8. Flash & Stage into inactive slot
        let target_slot = self.inactive_slot();
        {
            let inactive_meta = self.inactive_slot_meta_mut();
            inactive_meta.version = header.version;
            inactive_meta.status = SlotStatus::Testing;
            inactive_meta.boot_tries_left = 3;
            inactive_meta.digest = header.payload_sha256;
        }

        // Set staged slot as active for subsequent test boot
        self.active_slot = target_slot;

        // Durable commit: persist slot metadata + boot-tries before returning so
        // the staged image survives a reboot into the new slot.
        self.persist()?;

        Ok(target_slot)
    }

    /// Marks the active slot as Valid after userland successfully reaches online status.
    pub fn commit_successful_boot(&mut self, cap_table: &CapabilityTable) -> Result<(), OtaError> {
        cap_table
            .check(CapabilityType::OtaUpdateManagement, Rights::WRITE)
            .map_err(|_| OtaError::CapabilityDenied)?;

        let meta = self.active_slot_meta_mut();
        if meta.status == SlotStatus::Testing {
            meta.status = SlotStatus::Valid;
            meta.boot_tries_left = 3;
        }
        self.persist()?;
        Ok(())
    }

    /// Handles a boot failure or watchdog crash during slot verification.
    /// Decrements tries counter and autonomously triggers rollback if tries reach 0.
    pub fn handle_boot_failure(&mut self) -> PartitionSlot {
        let meta = self.active_slot_meta_mut();
        meta.boot_tries_left = meta.boot_tries_left.saturating_sub(1);

        if meta.boot_tries_left == 0 {
            meta.status = SlotStatus::Corrupted;
            // Autonomous Rollback to previous working slot!
            self.active_slot = self.active_slot.alternate();
        }

        // Persist the decremented boot-tries (and any rollback) so the watchdog
        // state survives a hard power cycle. Best-effort: the in-RAM rollback
        // decision above still stands if the store is unavailable.
        let _ = self.persist();

        self.active_slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto;
    use alloc::rc::Rc;
    use core::cell::RefCell;

    /// In-memory persistent backend used only by tests. The shared `Rc` handle
    /// lets a test drop the manager (simulating a reboot) and reconstruct a new
    /// manager over the same backing, proving state actually persisted.
    #[derive(Clone, Default)]
    struct MemStore {
        backing: Rc<RefCell<Option<PersistedOtaState>>>,
    }

    impl MemStore {
        fn handle(&self) -> Rc<RefCell<Option<PersistedOtaState>>> {
            self.backing.clone()
        }
    }

    impl OtaStore for MemStore {
        fn load(&mut self) -> Result<Option<PersistedOtaState>, OtaError> {
            Ok(self.backing.borrow().clone())
        }

        fn save(&mut self, state: &PersistedOtaState) -> Result<(), OtaError> {
            *self.backing.borrow_mut() = Some(state.clone());
            Ok(())
        }
    }

    const TEST_BOARD: [u8; 16] = *b"PINEPHONE_A64\0\0\0";
    const TEST_SEED: [u8; 32] = [
        0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E, 0x0A, 0x0D, 0x00,
        0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E, 0x0A, 0x0D, 0x00, 0x0D, 0x0E, 0x0A, 0x0D, 0x00,
        0x0D, 0x0E,
    ];

    fn signed_header(version: u32, payload: &[u8], pubkey: &[u8; 32]) -> OtaPackageHeader {
        let digest = crypto::sha256(payload);
        let mut hdr = OtaPackageHeader {
            magic: OTA_MAGIC,
            target_arch: ARCH_AARCH64,
            target_board: TEST_BOARD,
            version,
            payload_len: payload.len() as u32,
            payload_sha256: digest,
            signature: [0u8; 64],
        };
        let preimage = hdr.compute_signature_preimage();
        hdr.signature = crypto::sign_ed25519_signature(&TEST_SEED, &preimage);
        let _ = pubkey;
        hdr
    }

    fn write_caps() -> CapabilityTable {
        let mut caps = CapabilityTable::new();
        caps.mint(CapabilityType::OtaUpdateManagement, Rights::WRITE, None);
        caps
    }

    #[test]
    fn stage_persists_across_reconstruction() {
        let pubkey = crypto::ed25519_public_key(&TEST_SEED);
        let payload = b"Inter OS AArch64 OTA v5.5.0 durable staging payload";

        let store = MemStore::default();
        let handle = store.handle();
        let mut mgr =
            OtaManager::with_store(store, 540, pubkey, TEST_BOARD).expect("fresh store init");
        assert_eq!(mgr.active_slot, PartitionSlot::SlotA);

        let hdr = signed_header(550, payload, &pubkey);
        let staged = mgr
            .verify_and_stage_update(&hdr, payload, &write_caps())
            .expect("staging must succeed");
        assert_eq!(staged, PartitionSlot::SlotB);

        // Simulate reboot: drop the manager, reconstruct over the same backing.
        drop(mgr);
        let mgr2 =
            OtaManager::with_store(MemStore { backing: handle }, 0, pubkey, TEST_BOARD)
                .expect("reload must succeed");

        assert_eq!(mgr2.active_slot, PartitionSlot::SlotB);
        assert_eq!(mgr2.slot_b.version, 550);
        assert_eq!(mgr2.slot_b.status, SlotStatus::Testing);
        assert_eq!(mgr2.slot_b.boot_tries_left, 3);
        assert_eq!(mgr2.slot_b.digest, hdr.payload_sha256);
    }

    #[test]
    fn boot_failure_decrement_persists() {
        let pubkey = crypto::ed25519_public_key(&TEST_SEED);
        let payload = b"Inter OS AArch64 OTA v5.5.0 watchdog persistence payload";

        let store = MemStore::default();
        let handle = store.handle();
        let mut mgr = OtaManager::with_store(store, 540, pubkey, TEST_BOARD).expect("init");

        let hdr = signed_header(550, payload, &pubkey);
        mgr.verify_and_stage_update(&hdr, payload, &write_caps())
            .expect("staging must succeed");
        assert_eq!(mgr.slot_b.boot_tries_left, 3);

        // One failed boot decrements to 2 and stays on the staged slot.
        assert_eq!(mgr.handle_boot_failure(), PartitionSlot::SlotB);
        assert_eq!(mgr.slot_b.boot_tries_left, 2);

        // Reboot: the decremented counter must be read back from the store.
        drop(mgr);
        let mgr2 =
            OtaManager::with_store(MemStore { backing: handle }, 0, pubkey, TEST_BOARD)
                .expect("reload must succeed");

        assert_eq!(mgr2.active_slot, PartitionSlot::SlotB);
        assert_eq!(mgr2.slot_b.boot_tries_left, 2);
        assert_eq!(mgr2.slot_b.status, SlotStatus::Testing);
    }
}
