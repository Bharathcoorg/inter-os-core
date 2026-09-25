//! Cryptographic Package Revocation & Key Rotation Subsystem
//!
//! Maintains the Certificate Revocation List (CRL) and Key Rotation Registry.
//! Prevents compromised developer signing keys and malicious package updates
//! from executing in the microkernel environment.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use spin::Mutex;
use crate::crypto::HmacSha256;
use crate::serial_println;

/// Reason for revoking a package or developer signing key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationReason {
    KeyCompromised = 1,
    MaliciousBehavior = 2,
    SupersededByRotation = 3,
    Expired = 4,
}

/// A cryptographic revocation entry in the CRL.
#[derive(Debug, Clone)]
pub struct RevocationRecord {
    pub target_id: String,
    pub reason: RevocationReason,
    pub revoked_at_epoch: u64,
    pub issuer: String,
}

/// Cryptographic Key Rotation Certificate.
/// Allows a developer to rotate their master signing key by proving
/// possession of the prior key.
#[derive(Debug, Clone)]
pub struct KeyRotationProof {
    pub publisher: String,
    pub old_key: [u8; 32],
    pub new_key: [u8; 32],
    pub epoch: u64,
    pub proof_signature: [u8; 32],
}

impl KeyRotationProof {
    /// Computes the expected HMAC-SHA256 signature for a key transition.
    pub fn compute_signature(old_key: &[u8; 32], new_key: &[u8; 32], epoch: u64) -> [u8; 32] {
        let mut msg = [0u8; 40];
        msg[..32].copy_from_slice(new_key);
        msg[32..40].copy_from_slice(&epoch.to_le_bytes());
        HmacSha256::compute(old_key, &msg)
    }

    /// Verifies the key rotation transition proof.
    pub fn verify(&self) -> bool {
        let expected = Self::compute_signature(&self.old_key, &self.new_key, self.epoch);
        crate::crypto::constant_time_eq_32(&expected, &self.proof_signature)
    }
}

/// Global Revocation & Key Rotation Registry.
pub struct RevocationRegistry {
    pub revoked_packages: BTreeMap<String, RevocationRecord>,
    pub revoked_keys: BTreeMap<[u8; 32], RevocationRecord>,
    pub rotated_keys: BTreeMap<String, [u8; 32]>,
    pub trusted_keys: BTreeMap<[u8; 32], String>,
}

impl RevocationRegistry {
    pub const fn new() -> Self {
        Self {
            revoked_packages: BTreeMap::new(),
            revoked_keys: BTreeMap::new(),
            rotated_keys: BTreeMap::new(),
            trusted_keys: BTreeMap::new(),
        }
    }

    /// Revokes a specific package ID across the entire OS.
    pub fn revoke_package(
        &mut self,
        package_id: &str,
        reason: RevocationReason,
        epoch: u64,
        issuer: &str,
    ) {
        let rec = RevocationRecord {
            target_id: package_id.to_string(),
            reason,
            revoked_at_epoch: epoch,
            issuer: issuer.to_string(),
        };
        self.revoked_packages.insert(package_id.to_string(), rec);
        serial_println!(
            "[REVOCATION] Package '{}' placed on CRL (Reason: {:?}).",
            package_id,
            reason
        );
    }

    /// Revokes a developer public key fingerprint.
    pub fn revoke_key(
        &mut self,
        key: &[u8; 32],
        reason: RevocationReason,
        epoch: u64,
        issuer: &str,
    ) {
        let rec = RevocationRecord {
            target_id: String::from("KEY_REVOCATION"),
            reason,
            revoked_at_epoch: epoch,
            issuer: issuer.to_string(),
        };
        self.revoked_keys.insert(*key, rec);
        serial_println!(
            "[REVOCATION] Key 0x{:02X}{:02X}... placed on CRL (Reason: {:?}).",
            key[0],
            key[1],
            reason
        );
    }

    /// Returns true if a package ID is on the active CRL.
    pub fn is_package_revoked(&self, package_id: &str) -> bool {
        self.revoked_packages.contains_key(package_id)
    }

    /// Returns true if a public key fingerprint is revoked.
    pub fn is_key_revoked(&self, key: &[u8; 32]) -> bool {
        self.revoked_keys.contains_key(key)
    }

    /// Registers a publisher public key in the device trust store.
    /// Provisioning this key is a privileged device lifecycle operation.
    pub fn trust_key(&mut self, key: [u8; 32], publisher: &str) {
        self.trusted_keys.insert(key, publisher.to_string());
    }

    /// Returns true only for an explicitly provisioned publisher key.
    pub fn is_key_trusted(&self, key: &[u8; 32]) -> bool {
        self.trusted_keys.contains_key(key) && !self.is_key_revoked(key)
    }

    /// Processes an attested key rotation proof.
    pub fn apply_key_rotation(&mut self, proof: &KeyRotationProof) -> Result<(), &'static str> {
        if !proof.verify() {
            return Err("Invalid key rotation cryptographic proof");
        }
        if self.is_key_revoked(&proof.old_key) {
            return Err("Old key is revoked; cannot be rotated");
        }

        self.rotated_keys.insert(proof.publisher.clone(), proof.new_key);
        self.trust_key(proof.new_key, &proof.publisher);
        self.revoke_key(
            &proof.old_key,
            RevocationReason::SupersededByRotation,
            proof.epoch,
            &proof.publisher,
        );
        serial_println!(
            "[KEY ROTATION] Publisher '{}' rotated key successfully to epoch {}.",
            proof.publisher,
            proof.epoch
        );
        Ok(())
    }
}

pub static REVOCATION_REGISTRY: Mutex<RevocationRegistry> = Mutex::new(RevocationRegistry::new());


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trust_and_revocation_invariants() {
        let key = [7u8; 32];
        let mut r = RevocationRegistry::new();
        assert!(!r.is_key_trusted(&key));
        r.trust_key(key, "test-publisher");
        assert!(r.is_key_trusted(&key));
        r.revoke_key(&key, RevocationReason::KeyCompromised, 10, "root");
        assert!(!r.is_key_trusted(&key));
    }
}
