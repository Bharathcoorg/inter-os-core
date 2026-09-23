//! State Commitment & Cryptographic Chain Representation
//!
//! Encapsulates an attested state transition link in the device's state log.
//! Each commitment binds the previous state root to the new state root via the
//! delta's Merkle root, signed by the device's sovereign identity key.

use alloc::vec::Vec;
use crate::crypto::Sha256;
use crate::identity::{IdentityVault, SovereignDid};

/// Attested state commitment record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateCommitment {
    pub sequence: u64,
    pub prev_state_root: [u8; 32],
    pub delta_merkle_root: [u8; 32],
    pub new_state_root: [u8; 32],
    pub timestamp_ticks: u64,
    pub device_did: SovereignDid,
    pub signature: [u8; 32],
}

impl StateCommitment {
    /// Serializes the commitment fields (excluding signature) into canonical bytes for signing.
    pub fn canonical_payload(&self) -> Vec<u8> {
        let did_bytes = self.device_did.as_str().as_bytes();
        let mut buf = Vec::with_capacity(8 + 32 + 32 + 32 + 8 + did_bytes.len());
        buf.extend_from_slice(&self.sequence.to_le_bytes());
        buf.extend_from_slice(&self.prev_state_root);
        buf.extend_from_slice(&self.delta_merkle_root);
        buf.extend_from_slice(&self.new_state_root);
        buf.extend_from_slice(&self.timestamp_ticks.to_le_bytes());
        buf.extend_from_slice(did_bytes);
        buf
    }

    /// Derives the new state root from previous state root and delta Merkle root.
    pub fn derive_new_root(prev_root: &[u8; 32], delta_root: &[u8; 32]) -> [u8; 32] {
        let mut combined = [0u8; 64];
        combined[..32].copy_from_slice(prev_root);
        combined[32..].copy_from_slice(delta_root);
        Sha256::digest(&combined)
    }

    /// Verifies the cryptographic attestation of this commitment using the vault.
    pub fn verify(&self, vault: &IdentityVault) -> bool {
        // 1. Verify that new_state_root matches derivation
        let expected_root = Self::derive_new_root(&self.prev_state_root, &self.delta_merkle_root);
        if self.new_state_root != expected_root {
            return false;
        }

        // 2. Verify signature over canonical payload
        let payload = self.canonical_payload();
        vault.verify_signature(&payload, &self.signature)
    }
}
