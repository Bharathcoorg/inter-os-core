//! Inter OS — AArch64 Sovereign OTA Microkernel Updates & A/B Partition Rollback Engine
//!
//! Non-Negotiable Principle #1: User owns the machine.
//! Non-Negotiable Principle #17: Private state remains protected; public assertions use cryptographic proofs.
//!
//! Provides bare-metal A/B dual partition slot management, cryptographic update verification,
//! anti-rollback monotonic version enforcement, and watchdog boot counter rollback protection.

extern crate alloc;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::crypto::{constant_time_eq_32, sha256, verify_ed25519_signature};

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

/// Dual Partition A/B OTA & Autonomous Rollback Manager.
pub struct OtaManager {
    pub active_slot: PartitionSlot,
    pub slot_a: SlotMetadata,
    pub slot_b: SlotMetadata,
    pub publisher_pubkey: [u8; 32],
    pub device_board_id: [u8; 16],
}

impl OtaManager {
    /// Creates a new OTA manager with Slot A as initially active and valid.
    pub fn new(current_version: u32, publisher_key: [u8; 32], board_id: [u8; 16]) -> Self {
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
        }
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

        self.active_slot
    }
}
