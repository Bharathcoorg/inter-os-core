//! Persistent package/key revocation and publisher key-rotation policy.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use spin::Mutex;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use crate::serial_println;

const MAX_PERSISTED_RECORDS: usize = 24;
const RECORD_SIZE: usize = 105;
const REVOCATION_MAGIC: &[u8; 4] = b"RVK1";
const TRUST_MAGIC: &[u8; 4] = b"TRS1";
const TRUST_RECORD_SIZE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationReason {
    KeyCompromised = 1,
    MaliciousBehavior = 2,
    SupersededByRotation = 3,
    Expired = 4,
}

#[derive(Debug, Clone)]
pub struct RevocationRecord {
    pub target_id: String,
    pub reason: RevocationReason,
    pub revoked_at_epoch: u64,
    pub issuer: String,
}

#[derive(Debug, Clone)]
pub struct KeyRotationProof {
    pub publisher: String,
    pub old_key: [u8; 32],
    pub new_key: [u8; 32],
    pub epoch: u64,
    pub proof_signature: [u8; 64],
}

impl KeyRotationProof {
    fn message(&self) -> [u8; 40] {
        let mut msg = [0u8; 40];
        msg[..32].copy_from_slice(&self.new_key);
        msg[32..40].copy_from_slice(&self.epoch.to_le_bytes());
        msg
    }

    /// Verifies a rotation authorization signed by the old publisher private key.
    pub fn verify(&self) -> bool {
        let Ok(key) = VerifyingKey::from_bytes(&self.old_key) else { return false; };
        let signature = Signature::from_bytes(&self.proof_signature);
        key.verify(&self.message(), &signature).is_ok()
    }
}

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

    fn persist(&self) -> Result<(), ()> {
        let mut buf = [0u8; 4096];
        buf[0..4].copy_from_slice(REVOCATION_MAGIC);
        let mut pos = 5usize;
        let mut count = 0usize;

        for (id, rec) in self.revoked_packages.iter().take(MAX_PERSISTED_RECORDS) {
            if pos + RECORD_SIZE > buf.len() { break; }
            buf[pos..pos + 32].fill(0);
            let idb = id.as_bytes();
            let n = idb.len().min(32);
            buf[pos..pos + n].copy_from_slice(&idb[..n]);
            pos += 32;
            buf[pos] = rec.reason as u8; pos += 1;
            buf[pos..pos + 8].copy_from_slice(&rec.revoked_at_epoch.to_le_bytes()); pos += 8;
            buf[pos..pos + 32].fill(0);
            let ib = rec.issuer.as_bytes();
            let n = ib.len().min(32);
            buf[pos..pos + n].copy_from_slice(&ib[..n]);
            pos += 32;
            buf[pos..pos + 32].fill(0); pos += 32;
            count += 1;
        }

        for (key, rec) in self.revoked_keys.iter().take(MAX_PERSISTED_RECORDS.saturating_sub(count)) {
            if pos + RECORD_SIZE > buf.len() { break; }
            buf[pos..pos + 32].copy_from_slice(key);
            pos += 32;
            buf[pos] = 0x80 | rec.reason as u8; pos += 1;
            buf[pos..pos + 8].copy_from_slice(&rec.revoked_at_epoch.to_le_bytes()); pos += 8;
            buf[pos..pos + 32].fill(0);
            let ib = rec.issuer.as_bytes();
            let n = ib.len().min(32);
            buf[pos..pos + n].copy_from_slice(&ib[..n]);
            pos += 32;
            buf[pos..pos + 32].fill(0); pos += 32;
            count += 1;
        }

        buf[4] = count as u8;
        crate::storage::persistent::commit_kind(&buf[..pos], 2).map(|_| ()).map_err(|_| ())
    }

    pub fn restore(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let Ok(Some((_generation, len))) =
            crate::storage::persistent::load_latest_kind(&mut buf, 2) else { return false; };
        if len < 5 || &buf[..4] != REVOCATION_MAGIC { return false; }

        self.revoked_packages.clear();
        self.revoked_keys.clear();
        let count = (buf[4] as usize).min(MAX_PERSISTED_RECORDS);
        let mut pos = 5usize;
        for _ in 0..count {
            if pos + RECORD_SIZE > len { return false; }
            let target = &buf[pos..pos + 32];
            let target_len = target.iter().position(|b| *b == 0).unwrap_or(32);
            pos += 32;
            let kind = buf[pos]; pos += 1;
            let epoch = u64::from_le_bytes([
                buf[pos],buf[pos+1],buf[pos+2],buf[pos+3],
                buf[pos+4],buf[pos+5],buf[pos+6],buf[pos+7],
            ]); pos += 8;
            let issuer_raw = &buf[pos..pos + 32];
            let issuer_len = issuer_raw.iter().position(|b| *b == 0).unwrap_or(32);
            let issuer = match core::str::from_utf8(&issuer_raw[..issuer_len]) {
                Ok(v) => v.to_string(), Err(_) => return false
            };
            pos += 32;
            pos += 32;
            let reason = match kind & 0x7F {
                1 => RevocationReason::KeyCompromised,
                2 => RevocationReason::MaliciousBehavior,
                3 => RevocationReason::SupersededByRotation,
                4 => RevocationReason::Expired,
                _ => return false,
            };

            if kind & 0x80 == 0 {
                let id = match core::str::from_utf8(&target[..target_len]) {
                    Ok(v) => v.to_string(), Err(_) => return false
                };
                self.revoked_packages.insert(id.clone(), RevocationRecord {
                    target_id: id, reason, revoked_at_epoch: epoch, issuer,
                });
            } else if kind & 0x80 != 0 {
                let mut key = [0u8; 32];
                key.copy_from_slice(target);
                self.revoked_keys.insert(key, RevocationRecord {
                    target_id: String::from("KEY_REVOCATION"),
                    reason, revoked_at_epoch: epoch, issuer,
                });
            } else { return false; }
        }
        true
    }

    fn persist_trusted_keys(&self) -> Result<(), ()> {
        let mut buf = [0u8; 4096];
        buf[0..4].copy_from_slice(TRUST_MAGIC);
        let count = self.trusted_keys.len().min(MAX_PERSISTED_RECORDS);
        buf[4] = count as u8;
        let mut pos = 5usize;
        for (key, publisher) in self.trusted_keys.iter().take(count) {
            if pos + TRUST_RECORD_SIZE > buf.len() { break; }
            buf[pos..pos + 32].copy_from_slice(key);
            pos += 32;
            buf[pos..pos + 32].fill(0);
            let bytes = publisher.as_bytes();
            let n = bytes.len().min(32);
            buf[pos..pos + n].copy_from_slice(&bytes[..n]);
            pos += 32;
        }
        crate::storage::persistent::commit_kind(&buf[..pos], 3).map(|_| ()).map_err(|_| ())
    }

    fn restore_trusted_keys(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let Ok(Some((_generation, len))) =
            crate::storage::persistent::load_latest_kind(&mut buf, 3) else { return false; };
        if len < 5 || &buf[..4] != TRUST_MAGIC { return false; }
        self.trusted_keys.clear();
        let count = (buf[4] as usize).min(MAX_PERSISTED_RECORDS);
        let mut pos = 5usize;
        for _ in 0..count {
            if pos + TRUST_RECORD_SIZE > len { return false; }
            let mut key = [0u8; 32];
            key.copy_from_slice(&buf[pos..pos + 32]);
            pos += 32;
            let raw = &buf[pos..pos + 32];
            let end = raw.iter().position(|b| *b == 0).unwrap_or(32);
            let publisher = match core::str::from_utf8(&raw[..end]) {
                Ok(v) => v.to_string(),
                Err(_) => return false,
            };
            pos += 32;
            self.trusted_keys.insert(key, publisher);
        }
        true
    }

    pub fn seed(&mut self) {
        if self.restore() {
            serial_println!("[REVOCATION] persistent CRL restored.");
        }
        if self.restore_trusted_keys() {
            serial_println!("[TRUST] persistent publisher trust roots restored.");
        }
    }

    pub fn revoke_package(&mut self, package_id: &str, reason: RevocationReason, epoch: u64, issuer: &str) {
        let rec = RevocationRecord {
            target_id: package_id.to_string(), reason,
            revoked_at_epoch: epoch, issuer: issuer.to_string(),
        };
        self.revoked_packages.insert(package_id.to_string(), rec);
        let _ = self.persist();
        serial_println!("[REVOCATION] Package '{}' placed on CRL (Reason: {:?}).", package_id, reason);
    }

    pub fn revoke_key(&mut self, key: &[u8; 32], reason: RevocationReason, epoch: u64, issuer: &str) {
        let rec = RevocationRecord {
            target_id: String::from("KEY_REVOCATION"), reason,
            revoked_at_epoch: epoch, issuer: issuer.to_string(),
        };
        self.revoked_keys.insert(*key, rec);
        let _ = self.persist();
        serial_println!("[REVOCATION] Key 0x{:02X}{:02X}... placed on CRL (Reason: {:?}).", key[0], key[1], reason);
    }

    pub fn is_package_revoked(&self, package_id: &str) -> bool {
        self.revoked_packages.contains_key(package_id)
    }

    pub fn is_key_revoked(&self, key: &[u8; 32]) -> bool {
        self.revoked_keys.contains_key(key)
    }

    /// Registers a publisher public key in the device trust store.
    /// Provisions a publisher trust root from a secure boot/provisioning path.
    /// This method intentionally has no userspace syscall wrapper.
    pub fn trust_key(&mut self, key: [u8; 32], publisher: &str) {
        self.trusted_keys.insert(key, publisher.to_string());
        let _ = self.persist_trusted_keys();
    }

    /// Returns true only for an explicitly provisioned publisher key.
    pub fn is_key_trusted(&self, key: &[u8; 32]) -> bool {
        self.trusted_keys.contains_key(key) && !self.is_key_revoked(key)
    }

    pub fn apply_key_rotation(&mut self, proof: &KeyRotationProof) -> Result<(), &'static str> {
        if !proof.verify() { return Err("Invalid key rotation cryptographic proof"); }
        if !self.is_key_trusted(&proof.old_key) { return Err("Old key is not a trusted publisher root"); }
        if self.is_key_revoked(&proof.old_key) { return Err("Old key is revoked; cannot be rotated"); }

        self.rotated_keys.insert(proof.publisher.clone(), proof.new_key);
        self.trust_key(proof.new_key, &proof.publisher);
        self.revoke_key(&proof.old_key, RevocationReason::SupersededByRotation, proof.epoch, &proof.publisher);
        serial_println!("[KEY ROTATION] Publisher '{}' rotated key successfully to epoch {}.", proof.publisher, proof.epoch);
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
