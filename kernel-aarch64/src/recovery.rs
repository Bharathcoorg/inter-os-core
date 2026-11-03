// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS — AArch64 Sovereign Decentralized Identity Peer Attestation & Social Recovery Enclave
//!
//! Non-Negotiable Principle #1: User owns the machine.
//! Non-Negotiable Principle #17: Private state remains protected; public assertions use cryptographic proofs.
//!
//! Provides a pure `no_std` decentralized M-of-N threshold social recovery scheme.
//! When a physical smartphone is lost, damaged, or undergoing sovereign device migration,
//! identity recovery is achieved via signed cryptographic attestations from authorized
//! peer guardians (friends, family, or offline backup keys) without any centralized
//! cloud server, SMS gateway, or trusted third party.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::crypto::{constant_time_eq_32, HmacSha256};

/// Status of an authorized recovery guardian.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GuardianStatus {
    Active,
    Revoked,
}

/// An authorized sovereign peer guardian registered in the recovery enclave.
#[derive(Clone, Debug)]
pub struct RecoveryGuardian {
    pub did: String,
    pub guardian_key: [u8; 32],
    pub status: GuardianStatus,
}

/// Errors returned by the Social Recovery Enclave.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    CapabilityDenied,
    InvalidThreshold,
    GuardianAlreadyExists,
    GuardianNotFound,
    GuardianNotActive,
    InvalidSignature,
    DuplicateAttestation,
    ClaimNotFound,
    QuorumNotReached,
    TimelockNotExpired,
    ClaimCancelled,
    ClaimAlreadyFinalized,
    OwnerAuthFailed,
}

/// Configuration policy governing M-of-N social recovery.
#[derive(Clone, Debug)]
pub struct RecoveryPolicy {
    pub threshold: usize,
    pub delay_seconds: u64,
    pub guardians: Vec<RecoveryGuardian>,
}

impl RecoveryPolicy {
    /// Creates a new recovery policy with an M-of-N threshold and timelock delay.
    pub fn new(threshold: usize, delay_seconds: u64) -> Self {
        Self {
            threshold,
            delay_seconds,
            guardians: Vec::new(),
        }
    }

    /// Adds a new peer guardian to the policy.
    pub fn add_guardian(&mut self, did: &str, key: [u8; 32]) -> Result<(), RecoveryError> {
        if self.guardians.iter().any(|g| g.did == did) {
            return Err(RecoveryError::GuardianAlreadyExists);
        }
        self.guardians.push(RecoveryGuardian {
            did: String::from(did),
            guardian_key: key,
            status: GuardianStatus::Active,
        });
        Ok(())
    }

    /// Revokes an existing peer guardian.
    pub fn revoke_guardian(&mut self, did: &str) -> Result<(), RecoveryError> {
        if let Some(guardian) = self.guardians.iter_mut().find(|g| g.did == did) {
            guardian.status = GuardianStatus::Revoked;
            Ok(())
        } else {
            Err(RecoveryError::GuardianNotFound)
        }
    }

    /// Returns the number of currently active guardians.
    pub fn active_guardian_count(&self) -> usize {
        self.guardians.iter().filter(|g| g.status == GuardianStatus::Active).count()
    }
}

/// State of an active identity recovery claim.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClaimState {
    Initiated,
    QuorumReached,
    Timelocked,
    Finalized,
    Cancelled,
}

/// Cryptographic attestation provided by an authorized guardian.
#[derive(Clone, Debug)]
pub struct GuardianAttestation {
    pub guardian_did: String,
    pub signature: [u8; 32],
}

/// An active identity recovery claim initiated for a target DID.
#[derive(Clone, Debug)]
pub struct RecoveryClaim {
    pub claim_id: u64,
    pub target_did: String,
    pub new_owner_key: [u8; 32],
    pub nonce: u64,
    pub created_timestamp: u64,
    pub attestations: Vec<GuardianAttestation>,
    pub state: ClaimState,
}

/// Decentralized Identity Peer Attestation & Social Recovery Enclave.
pub struct SocialRecoveryEnclave {
    pub policy: RecoveryPolicy,
    pub active_claim: Option<RecoveryClaim>,
    pub next_claim_id: u64,
}

impl SocialRecoveryEnclave {
    /// Creates a new enclave instance with the specified policy.
    pub fn new(policy: RecoveryPolicy) -> Self {
        Self {
            policy,
            active_claim: None,
            next_claim_id: 1,
        }
    }

    /// Initiates a new identity recovery claim to rotate to `new_owner_key`.
    pub fn initiate_claim(
        &mut self,
        target_did: &str,
        new_owner_key: [u8; 32],
        now: u64,
        nonce: u64,
        cap_table: &CapabilityTable,
    ) -> Result<u64, RecoveryError> {
        // Enforce Zero Ambient Authority: require CapabilityType::SocialRecoveryEnclave with Rights::INVOKE
        cap_table
            .check(CapabilityType::SocialRecoveryEnclave, Rights::INVOKE)
            .map_err(|_| RecoveryError::CapabilityDenied)?;

        if self.policy.active_guardian_count() < self.policy.threshold {
            return Err(RecoveryError::InvalidThreshold);
        }

        let claim_id = self.next_claim_id;
        self.next_claim_id += 1;

        self.active_claim = Some(RecoveryClaim {
            claim_id,
            target_did: String::from(target_did),
            new_owner_key,
            nonce,
            created_timestamp: now,
            attestations: Vec::new(),
            state: ClaimState::Initiated,
        });

        Ok(claim_id)
    }

    /// Constructs the deterministic attestation payload for signature verification.
    pub fn compute_attestation_payload(
        claim_id: u64,
        target_did: &str,
        new_owner_key: &[u8; 32],
        nonce: u64,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&claim_id.to_be_bytes());
        payload.extend_from_slice(target_did.as_bytes());
        payload.extend_from_slice(new_owner_key);
        payload.extend_from_slice(&nonce.to_be_bytes());
        payload
    }

    /// Submits a signed cryptographic attestation from a peer guardian.
    pub fn submit_guardian_attestation(
        &mut self,
        claim_id: u64,
        guardian_did: &str,
        signature: [u8; 32],
        _now: u64,
        cap_table: &CapabilityTable,
    ) -> Result<ClaimState, RecoveryError> {
        // Enforce Zero Ambient Authority: require CapabilityType::SocialRecoveryEnclave with Rights::WRITE
        cap_table
            .check(CapabilityType::SocialRecoveryEnclave, Rights::WRITE)
            .map_err(|_| RecoveryError::CapabilityDenied)?;

        let claim = self.active_claim.as_mut().ok_or(RecoveryError::ClaimNotFound)?;
        if claim.claim_id != claim_id {
            return Err(RecoveryError::ClaimNotFound);
        }

        if claim.state == ClaimState::Cancelled {
            return Err(RecoveryError::ClaimCancelled);
        }
        if claim.state == ClaimState::Finalized {
            return Err(RecoveryError::ClaimAlreadyFinalized);
        }

        // Locate guardian in policy and ensure active
        let guardian = self
            .policy
            .guardians
            .iter()
            .find(|g| g.did == guardian_did)
            .ok_or(RecoveryError::GuardianNotFound)?;

        if guardian.status != GuardianStatus::Active {
            return Err(RecoveryError::GuardianNotActive);
        }

        // Check for duplicate attestation
        if claim.attestations.iter().any(|a| a.guardian_did == guardian_did) {
            return Err(RecoveryError::DuplicateAttestation);
        }

        // Cryptographically verify guardian's HMAC-SHA256 signature
        let payload = Self::compute_attestation_payload(
            claim.claim_id,
            &claim.target_did,
            &claim.new_owner_key,
            claim.nonce,
        );
        let expected_mac = HmacSha256::mac(&guardian.guardian_key, &payload);

        if !constant_time_eq_32(&signature, &expected_mac) {
            return Err(RecoveryError::InvalidSignature);
        }

        // Add verified attestation
        claim.attestations.push(GuardianAttestation {
            guardian_did: String::from(guardian_did),
            signature,
        });

        // Check if M-of-N quorum is reached
        if claim.attestations.len() >= self.policy.threshold {
            if self.policy.delay_seconds > 0 {
                claim.state = ClaimState::Timelocked;
            } else {
                claim.state = ClaimState::QuorumReached;
            }
        }

        Ok(claim.state)
    }

    /// Cancels a recovery claim by the authentic device owner before the timelock expires.
    /// Defends against malicious coercion or fraudulent recovery attempts.
    pub fn cancel_claim_by_owner(
        &mut self,
        claim_id: u64,
        owner_key: &[u8; 32],
        owner_mac: &[u8; 32],
        cap_table: &CapabilityTable,
    ) -> Result<(), RecoveryError> {
        cap_table
            .check(CapabilityType::SocialRecoveryEnclave, Rights::WRITE)
            .map_err(|_| RecoveryError::CapabilityDenied)?;

        let claim = self.active_claim.as_mut().ok_or(RecoveryError::ClaimNotFound)?;
        if claim.claim_id != claim_id {
            return Err(RecoveryError::ClaimNotFound);
        }

        // Verify owner signature over cancel command
        let mut cancel_payload = Vec::new();
        cancel_payload.extend_from_slice(b"CANCEL_RECOVERY");
        cancel_payload.extend_from_slice(&claim_id.to_be_bytes());
        let expected_mac = HmacSha256::mac(owner_key, &cancel_payload);

        if !constant_time_eq_32(owner_mac, &expected_mac) {
            return Err(RecoveryError::OwnerAuthFailed);
        }

        claim.state = ClaimState::Cancelled;
        Ok(())
    }

    /// Finalizes the recovery claim after the timelock window expires and quorum is reached.
    /// Rotates the sovereign identity root to the new owner key.
    pub fn finalize_recovery(
        &mut self,
        claim_id: u64,
        now: u64,
        cap_table: &CapabilityTable,
    ) -> Result<[u8; 32], RecoveryError> {
        cap_table
            .check(CapabilityType::SocialRecoveryEnclave, Rights::WRITE)
            .map_err(|_| RecoveryError::CapabilityDenied)?;

        let claim = self.active_claim.as_mut().ok_or(RecoveryError::ClaimNotFound)?;
        if claim.claim_id != claim_id {
            return Err(RecoveryError::ClaimNotFound);
        }

        if claim.state == ClaimState::Cancelled {
            return Err(RecoveryError::ClaimCancelled);
        }

        if claim.attestations.len() < self.policy.threshold {
            return Err(RecoveryError::QuorumNotReached);
        }

        if now < claim.created_timestamp.saturating_add(self.policy.delay_seconds) {
            return Err(RecoveryError::TimelockNotExpired);
        }

        claim.state = ClaimState::Finalized;
        Ok(claim.new_owner_key)
    }
}
