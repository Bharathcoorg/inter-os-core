//! Decentralized Package Revocation & Key Rotation Subsystem
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
}

impl RevocationRegistry {
    pub const fn new() -> Self {
        Self {
            revoked_packages: BTreeMap::new(),
            revoked_keys: BTreeMap::new(),
            rotated_keys: BTreeMap::new(),
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
            "[REVOCATION] Package '{}' placed on sovereign CRL (Reason: {:?}).",
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
            target_id: "dev_key_revoked".to_string(),
            reason,
            revoked_at_epoch: epoch,
            issuer: issuer.to_string(),
        };
        self.revoked_keys.insert(*key, rec);
        serial_println!(
            "[REVOCATION] Developer key revoked on sovereign CRL (Reason: {:?}).",
            reason
        );
    }

    /// Checks if a package ID has been revoked.
    pub fn is_package_revoked(&self, package_id: &str) -> bool {
        self.revoked_packages.contains_key(package_id)
    }

    /// Checks if a developer key has been revoked.
    pub fn is_key_revoked(&self, key: &[u8; 32]) -> bool {
        self.revoked_keys.contains_key(key)
    }

    /// Applies an authenticated key rotation proof.
    pub fn apply_key_rotation(&mut self, proof: &KeyRotationProof) -> Result<(), &'static str> {
        if !proof.verify() {
            return Err("KeyRotationSignatureInvalid");
        }

        // Place old key on revocation list (SupersededByRotation)
        self.revoke_key(
            &proof.old_key,
            RevocationReason::SupersededByRotation,
            proof.epoch,
            &proof.publisher,
        );

        // Store new active key
        self.rotated_keys.insert(proof.publisher.clone(), proof.new_key);
        serial_println!(
            "[KEY ROTATION] Successfully rotated signing key for publisher '{}'.",
            proof.publisher
        );
        Ok(())
    }

    /// Returns the active developer key for a publisher, taking rotations into account.
    pub fn get_active_publisher_key(&self, publisher: &str, default_key: &[u8; 32]) -> [u8; 32] {
        self.rotated_keys.get(publisher).copied().unwrap_or(*default_key)
    }
}

/// Global thread-safe instance of the Revocation & Key Rotation Registry.
pub static REVOCATION_REGISTRY: Mutex<RevocationRegistry> =
    Mutex::new(RevocationRegistry::new());

/// Initializes the revocation subsystem and verifies CRL checks and key rotation self-tests.
pub fn init() {
    let mut reg = REVOCATION_REGISTRY.lock();

    // 1. Initial State: Nominal, no packages revoked
    assert!(!reg.is_package_revoked("org.interos.calculator"));

    // 2. Test Revoking a compromised package
    reg.revoke_package(
        "rogue.malicious_crypto_miner",
        RevocationReason::MaliciousBehavior,
        1789843800,
        "did:inter:security:policy_core",
    );
    assert!(reg.is_package_revoked("rogue.malicious_crypto_miner"));
    assert!(!reg.is_package_revoked("org.interos.calculator"));

    // 3. Test Developer Key Rotation
    let old_dev_key = [0x55u8; 32];
    let new_dev_key = [0x77u8; 32];
    let epoch = 1789844000u64;

    let valid_sig = KeyRotationProof::compute_signature(&old_dev_key, &new_dev_key, epoch);
    let valid_proof = KeyRotationProof {
        publisher: "did:inter:developer:verified_partner".to_string(),
        old_key: old_dev_key,
        new_key: new_dev_key,
        epoch,
        proof_signature: valid_sig,
    };

    let rotate_res = reg.apply_key_rotation(&valid_proof);
    assert!(rotate_res.is_ok(), "Valid key rotation proof must succeed");

    // Old key must now be revoked
    assert!(reg.is_key_revoked(&old_dev_key));
    // New key is active
    let active_key = reg.get_active_publisher_key("did:inter:developer:verified_partner", &[0u8; 32]);
    assert_eq!(active_key, new_dev_key);

    // Tampered rotation proof must be rejected
    let mut tampered_proof = valid_proof.clone();
    tampered_proof.proof_signature[0] ^= 0xFF;
    let tampered_res = reg.apply_key_rotation(&tampered_proof);
    assert_eq!(tampered_res, Err("KeyRotationSignatureInvalid"));

    serial_println!(
        "[TEST] Decentralized CRL & Key Rotation: Anti-compromise revocation and authenticated rotation VERIFIED."
    );
}
