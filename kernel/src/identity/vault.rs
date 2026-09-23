//! In-Kernel Sovereign Identity Vault
//!
//! Safeguards the device's sovereign cryptographic root key, computes
//! decentralized identifiers, signs state delta commitments, and issues
//! zero-trust delegation tokens.
//!
//! Non-Negotiable Principle #1: User owns the machine.
//! Non-Negotiable Principle #17: Private state remains protected; public assertions use cryptographic proofs.

use alloc::vec::Vec;
use crate::crypto::{HmacSha256, Sha256, SHA256_DIGEST_LENGTH};
use super::did::SovereignDid;

/// Cryptographically attested capability delegation token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationToken {
    /// Subject DID granted the delegation.
    pub subject_did: SovereignDid,
    /// Authorized recipient Task ID.
    pub task_id: u64,
    /// Bitmask of permitted capability types.
    pub capability_mask: u32,
    /// Expiration hardware timer tick.
    pub expiry_ticks: u64,
    /// Cryptographic signature over token payload.
    pub signature: [u8; SHA256_DIGEST_LENGTH],
}

/// Sovereign Identity Vault holding private device credentials.
pub struct IdentityVault {
    master_seed: [u8; 32],
    public_key: [u8; 32],
    device_did: SovereignDid,
    tokens_issued: u64,
}

impl IdentityVault {
    /// Initializes the Identity Vault from a 32-byte master seed.
    pub fn new(seed: [u8; 32]) -> Self {
        let public_key = Sha256::digest(&seed);
        let device_did = SovereignDid::from_pubkey_hash(&public_key);
        Self {
            master_seed: seed,
            public_key,
            device_did,
            tokens_issued: 0,
        }
    }

    /// Returns the device's Sovereign DID.
    pub fn device_did(&self) -> &SovereignDid {
        &self.device_did
    }

    /// Returns the device's 32-byte public key hash.
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// Signs an arbitrary message using the vault's master seed (HMAC-SHA256).
    pub fn sign(&self, message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        HmacSha256::mac(&self.master_seed, message)
    }

    /// Verifies a signature against this device's master seed.
    pub fn verify_signature(&self, message: &[u8], signature: &[u8; SHA256_DIGEST_LENGTH]) -> bool {
        HmacSha256::verify(&self.master_seed, message, signature)
    }

    /// Issues an attested capability delegation token.
    pub fn issue_delegation_token(
        &mut self,
        task_id: u64,
        capability_mask: u32,
        duration_ticks: u64,
        current_ticks: u64,
    ) -> DelegationToken {
        let expiry_ticks = current_ticks.saturating_add(duration_ticks);

        // Serialize payload for signing: task_id (8) + capability_mask (4) + expiry_ticks (8) + did
        let mut payload = Vec::with_capacity(8 + 4 + 8 + self.device_did.as_str().len());
        payload.extend_from_slice(&task_id.to_le_bytes());
        payload.extend_from_slice(&capability_mask.to_le_bytes());
        payload.extend_from_slice(&expiry_ticks.to_le_bytes());
        payload.extend_from_slice(self.device_did.as_str().as_bytes());

        let signature = self.sign(&payload);
        self.tokens_issued = self.tokens_issued.saturating_add(1);

        DelegationToken {
            subject_did: self.device_did.clone(),
            task_id,
            capability_mask,
            expiry_ticks,
            signature,
        }
    }

    /// Verifies a delegation token's signature and expiration status.
    pub fn verify_delegation_token(&self, token: &DelegationToken, current_ticks: u64) -> bool {
        if current_ticks > token.expiry_ticks {
            return false;
        }

        let mut payload = Vec::with_capacity(8 + 4 + 8 + token.subject_did.as_str().len());
        payload.extend_from_slice(&token.task_id.to_le_bytes());
        payload.extend_from_slice(&token.capability_mask.to_le_bytes());
        payload.extend_from_slice(&token.expiry_ticks.to_le_bytes());
        payload.extend_from_slice(token.subject_did.as_str().as_bytes());

        self.verify_signature(&payload, &token.signature)
    }
}
