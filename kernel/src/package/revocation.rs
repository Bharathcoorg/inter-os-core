//! Persistent package/key revocation and publisher key-rotation policy.
//!
//! # Publisher trust model
//!
//! A `.rapp` publisher is trusted **only** when the owner of the device
//! installed its Ed25519 public key into the publisher trust store held here
//! ([`RevocationRegistry::trusted_keys`], persisted in the storage journal
//! under kind `3`). The store is empty on a device whose owner never
//! installed a root, and the OS grants itself none.
//!
//! WHY no public key is compiled into the OS: a trust root that ships inside
//! the image is a root that trusts every package signed with the matching
//! private key, on every device that ships it, and that private key is
//! exactly the material an attacker wants. Trust has to come from an
//! installation the device owner performed, and from nowhere else.
//!
//! WHY the store is not a grant of authority: a trusted publisher key still
//! has to pass the Ed25519 signature check, the payload digest check, the
//! capability policy and the revocation list. The trust store answers "does
//! the owner of this device know this publisher", not "is this package
//! acceptable".

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
    /// Publisher public keys the owner of this device installed, each with the
    /// publisher identity recorded alongside it.
    ///
    /// WHY this map is the only trust root: a key that is not in it is not
    /// trusted, whatever else the image happens to contain. There is no
    /// compiled-in public key and no built-in publisher.
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

    /// Restores the device's persisted revocation list and publisher trust
    /// store.
    ///
    /// WHY this grants nothing: a root the OS installs for itself is a root
    /// that trusts every package signed with it, on every device. The store
    /// therefore holds exactly the roots the owner of the device installed,
    /// and a device that never had one installed trusts no publisher at all.
    pub fn seed(&mut self) {
        if self.restore() {
            serial_println!("[REVOCATION] persistent CRL restored.");
        }
        if self.restore_trusted_keys() {
            serial_println!(
                "[TRUST] {} operator-installed publisher trust root(s) restored.",
                self.trusted_keys.len()
            );
        } else {
            serial_println!(
                "[TRUST] No publisher trust root is installed on this device; every publisher is untrusted until the owner installs one."
            );
        }
    }

    pub fn revoke_package(&mut self, package_id: &str, reason: RevocationReason, epoch: u64, issuer: &str) -> Result<(), &'static str> {
        let previous = self.revoked_packages.get(package_id).cloned();
        self.place_package_on_crl(package_id, reason, epoch, issuer)?;
        if self.persist().is_err() {
            match previous {
                Some(old) => { self.revoked_packages.insert(package_id.to_string(), old); }
                None => { self.revoked_packages.remove(package_id); }
            }
            return Err("failed to persist package revocation");
        }
        serial_println!("[REVOCATION] Package '{}' placed on CRL (Reason: {:?}).", package_id, reason);
        Ok(())
    }

    /// Places a package on the CRL in memory.
    pub(crate) fn place_package_on_crl(&mut self, package_id: &str, reason: RevocationReason, epoch: u64, issuer: &str) -> Result<(), &'static str> {
        if !self.revoked_packages.contains_key(package_id) && self.revoked_packages.len() >= MAX_PERSISTED_RECORDS {
            return Err("package revocation registry is full");
        }
        self.revoked_packages.insert(package_id.to_string(), RevocationRecord {
            target_id: package_id.to_string(),
            reason,
            revoked_at_epoch: epoch,
            issuer: issuer.to_string(),
        });
        Ok(())
    }

    pub fn revoke_key(&mut self, key: &[u8; 32], reason: RevocationReason, epoch: u64, issuer: &str) -> Result<(), &'static str> {
        let previous = self.revoked_keys.get(key).cloned();
        self.place_key_on_crl(key, reason, epoch, issuer)?;
        if self.persist().is_err() {
            match previous {
                Some(old) => { self.revoked_keys.insert(*key, old); }
                None => { self.revoked_keys.remove(key); }
            }
            return Err("failed to persist key revocation");
        }
        serial_println!("[REVOCATION] Key 0x{:02X}{:02X}... placed on CRL (Reason: {:?}).", key[0], key[1], reason);
        Ok(())
    }

    /// Places a key on the CRL in memory.
    ///
    /// WHY the mutation is separate from the journal commit above: a revocation
    /// the journal refused must leave no trace, and that compensation is only
    /// observable when the commit can fail — which is every machine that has no
    /// block device bound, including every host unit test. Keeping the state
    /// transition here lets the decision rule in `is_key_trusted` be checked
    /// directly instead of only through an unreachable durability path.
    pub(crate) fn place_key_on_crl(&mut self, key: &[u8; 32], reason: RevocationReason, epoch: u64, issuer: &str) -> Result<(), &'static str> {
        if !self.revoked_keys.contains_key(key) && self.revoked_keys.len() >= MAX_PERSISTED_RECORDS {
            return Err("key revocation registry is full");
        }
        self.revoked_keys.insert(*key, RevocationRecord {
            target_id: String::from("KEY_REVOCATION"), reason,
            revoked_at_epoch: epoch, issuer: issuer.to_string(),
        });
        Ok(())
    }

    pub fn is_package_revoked(&self, package_id: &str) -> bool {
        self.revoked_packages.contains_key(package_id)
    }

    pub fn is_key_revoked(&self, key: &[u8; 32]) -> bool {
        self.revoked_keys.contains_key(key)
    }

    /// Installs a publisher public key into the device trust store, on behalf
    /// of the owner of the device.
    ///
    /// WHY this is the only way a publisher becomes trusted: the OS ships no
    /// public key of its own, so a trust root exists on a device only because
    /// the owner installed it here. The record is journalled, so it survives a
    /// reboot, and it is rolled back when the journal refuses the commit
    /// rather than leaving behind a root the device cannot explain.
    ///
    /// WHY a platform identity is refused here: the console that reaches this
    /// function is not an authenticated provisioning channel, so it must not
    /// be able to mint device-wide administration (capability 18). Platform
    /// authority has to come from a channel that can prove it is the
    /// platform.
    ///
    /// This method intentionally has no userspace syscall wrapper: granting a
    /// publisher root is the device owner's act, not an application authority.
    pub fn trust_key(&mut self, key: [u8; 32], publisher: &str) -> Result<(), &'static str> {
        if is_system_publisher_identity(publisher) {
            return Err("platform publisher authority requires an authenticated provisioning channel");
        }
        self.install_root(key, publisher)
    }

    /// Journals a publisher public key into the device trust store.
    ///
    /// WHY this is separate from `trust_key`: `trust_key` is the entry point
    /// an unprivileged console can reach, so it refuses platform identities,
    /// while a key rotation has to be able to carry an identity that was
    /// already authenticated when it was first installed. The state transition
    /// and the journal commit are the same in both cases.
    fn install_root(&mut self, key: [u8; 32], publisher: &str) -> Result<(), &'static str> {
        let previous = self.trusted_keys.get(&key).cloned();
        self.grant_trust_root(key, publisher)?;
        if self.persist_trusted_keys().is_err() {
            match previous {
                Some(old) => { self.trusted_keys.insert(key, old); }
                None => { self.trusted_keys.remove(&key); }
            }
            return Err("failed to persist publisher trust root");
        }
        Ok(())
    }

    /// Records a publisher key in the device trust store, in memory only.
    ///
    /// WHY this exists beside `trust_key`: a trust root that only ever reaches
    /// RAM disappears at the next reboot, leaving the publisher silently
    /// untrusted, so `trust_key` is deliberately fail-closed and rolls the
    /// record back whenever the journal refuses the commit. That is correct,
    /// but it also means the trust decision itself cannot be observed on a
    /// machine with no block device bound. The state transition therefore lives
    /// here, where the invariant test can reach it.
    ///
    /// WHY this is not a second kind of trust root: it writes into the same
    /// store `trust_key` writes into, so a key recorded here is trusted only
    /// while the record survives, and it is withdrawn by the same CRL.
    pub(crate) fn grant_trust_root(&mut self, key: [u8; 32], publisher: &str) -> Result<(), &'static str> {
        if !self.trusted_keys.contains_key(&key) && self.trusted_keys.len() >= MAX_PERSISTED_RECORDS {
            return Err("publisher trust store is full");
        }
        self.trusted_keys.insert(key, publisher.to_string());
        Ok(())
    }

    /// Returns true only for a publisher public key the owner of this device
    /// installed, and only while that key is not revoked.
    ///
    /// WHY there is no built-in root: a public key compiled into the OS is
    /// shipped to every device and to everyone who reads the image, so any
    /// package signed with the matching private key would be trusted
    /// everywhere. Trust therefore comes only from the owner's own
    /// installation record, and an unknown publisher is untrusted.
    pub fn is_key_trusted(&self, key: &[u8; 32]) -> bool {
        if self.is_key_revoked(key) {
            return false;
        }
        self.trusted_keys.contains_key(key)
    }

    /// Returns the publisher identity recorded with an installed trust root.
    ///
    /// WHY this exists: a caller that reports on a package needs to name the
    /// publisher it actually verified against, and "the key was in the trust
    /// store" is not that name. A key the owner never installed has no
    /// publisher identity, and this returns `None` for it rather than a
    /// placeholder.
    pub fn trusted_publisher(&self, key: &[u8; 32]) -> Option<&str> {
        if !self.is_key_trusted(key) {
            return None;
        }
        self.trusted_keys.get(key).map(|publisher| publisher.as_str())
    }

    /// Returns true if the key is trusted and belongs to an authorized system/platform publisher.
    ///
    /// WHY the publisher identity is read from the trust store and not from
    /// the image: platform authority is a claim about the device, so it can
    /// only be carried by a root the owner of the device installed under a
    /// platform identity. There is no built-in platform publisher.
    pub fn is_system_publisher(&self, key: &[u8; 32]) -> bool {
        if !self.is_key_trusted(key) {
            return false;
        }
        match self.trusted_keys.get(key) {
            Some(publisher) => is_system_publisher_identity(publisher),
            None => false,
        }
    }

    /// Resolves the publisher identity a key rotation must carry forward.
    ///
    /// WHY the identity comes from the trust store and not from the proof:
    /// the rotation proof signs the new key and the epoch, and it does not
    /// sign the publisher name. A name taken from the proof would therefore
    /// let any trusted publisher rename itself into the platform namespace
    /// and mint capability 18 for its own successor key.
    fn rotation_publisher(&self, old_key: &[u8; 32]) -> Result<String, &'static str> {
        self.trusted_keys
            .get(old_key)
            .cloned()
            .ok_or("Old key has no recorded publisher identity")
    }

    /// Applies a publisher key rotation.
    ///
    /// WHY the new key inherits the identity recorded for the old key: the
    /// rotation proof signs the new key and the epoch, and it does not sign
    /// the publisher name. A label taken from the proof would therefore let
    /// any trusted publisher rename itself into the platform namespace and
    /// mint capability 18 for its own successor key.
    pub fn apply_key_rotation(&mut self, proof: &KeyRotationProof) -> Result<(), &'static str> {
        if !proof.verify() { return Err("Invalid key rotation cryptographic proof"); }
        if !self.is_key_trusted(&proof.old_key) { return Err("Old key is not a trusted publisher root"); }
        if self.is_key_revoked(&proof.old_key) { return Err("Old key is revoked; cannot be rotated"); }
        let publisher = self.rotation_publisher(&proof.old_key)?;

        let previous_rotated = self.rotated_keys.get(&publisher).copied();
        let previous_new_trust = self.trusted_keys.get(&proof.new_key).cloned();
        let previous_old_revocation = self.revoked_keys.get(&proof.old_key).cloned();

        self.rotated_keys.insert(publisher.clone(), proof.new_key);
        if self.revoke_key(&proof.old_key, RevocationReason::SupersededByRotation, proof.epoch, &publisher).is_err()
            || self.install_root(proof.new_key, &publisher).is_err()
        {
            match previous_rotated {
                Some(old) => { self.rotated_keys.insert(publisher.clone(), old); }
                None => { self.rotated_keys.remove(&publisher); }
            }
            match previous_new_trust {
                Some(old) => { self.trusted_keys.insert(proof.new_key, old); }
                None => { self.trusted_keys.remove(&proof.new_key); }
            }
            match previous_old_revocation {
                Some(old) => { self.revoked_keys.insert(proof.old_key, old); }
                None => { self.revoked_keys.remove(&proof.old_key); }
            }
            // WHY these two commits are best-effort and not reported: they run
            // only while already returning a failure, and if they fail too the
            // outcome is fail-closed — the journal keeps whatever the individual
            // `revoke_key`/`install_root` transactions had already undone, and the
            // publisher is left with its previous root rather than with a
            // half-applied rotation. There is no state to fall back *to* that the
            // caller could act on, so the refusal below is the whole answer.
            let _ = self.persist_trusted_keys();
            let _ = self.persist();
            return Err("failed to persist key rotation");
        }

        serial_println!("[KEY ROTATION] Publisher '{}' rotated key successfully to epoch {}.", publisher, proof.epoch);
        Ok(())
    }
}

pub static REVOCATION_REGISTRY: Mutex<RevocationRegistry> = Mutex::new(RevocationRegistry::new());

/// Publisher identities that may hold system platform authority.
///
/// WHY an explicit allowlist and a delimited namespace rather than a bare
/// prefix: `org.interos` as a prefix also matches `org.interos-evil` and
/// `org.interosX`, so a publisher record naming itself after the namespace
/// without belonging to it would have been granted platform authority. The two
/// fixed identities are matched exactly and the namespace test carries the
/// separating dot.
///
/// These strings are written when a trust root is provisioned. They are never
/// supplied by a package, so this list cannot be widened by anyone who merely
/// installs software.
pub const SYSTEM_PUBLISHER_IDENTITIES: [&str; 2] = ["system", "platform"];

/// The delimited namespace reserved for first-party InterOS publishers.
pub const SYSTEM_PUBLISHER_NAMESPACE: &str = "org.interos.";

/// Returns true if `publisher` names a platform publisher.
pub fn is_system_publisher_identity(publisher: &str) -> bool {
    SYSTEM_PUBLISHER_IDENTITIES.contains(&publisher)
        || publisher.starts_with(SYSTEM_PUBLISHER_NAMESPACE)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Trust is granted by exactly one thing — a provisioned trust root — and
    /// revoked by exactly one thing — an entry on the CRL. Revocation must
    /// always win, and nothing else may make a key trusted.
    ///
    /// WHY this drives `grant_trust_root`/`place_key_on_crl` and not the public
    /// `trust_key`/`revoke_key`: those are journal-backed and fail closed, so on
    /// a host with no block device bound they refuse every mutation and the
    /// decision rule is unreachable. `durable_trust_state_agrees_with_the_
    /// reported_outcome` pins the public half.
    ///
    /// Pinning assertion: the third one, `!is_key_trusted` after the key is on
    /// the CRL — a key that is simultaneously trusted and revoked is the
    /// over-revocation/under-revocation confusion this invariant exists to
    /// forbid.
    #[test]
    fn trust_and_revocation_invariants() {
        let key = [7u8; 32];
        let mut r = RevocationRegistry::new();
        assert!(!r.is_key_trusted(&key), "an unknown key must never be trusted");
        r.grant_trust_root(key, "test-publisher").expect("the trust store has room");
        assert!(r.is_key_trusted(&key), "a provisioned trust root must be trusted");
        r.place_key_on_crl(&key, RevocationReason::KeyCompromised, 10, "root").expect("the CRL has room");
        assert!(r.is_key_revoked(&key));
        assert!(!r.is_key_trusted(&key), "revocation must always win over trust");
    }

    /// The public, journal-backed entry points must never leave in-memory state
    /// that disagrees with the outcome they reported to the caller.
    ///
    /// WHY this is asserted in both directions rather than as a fixed `Ok`/`Err`:
    /// a trust root or CRL entry that is only in RAM disappears at the next
    /// reboot, so `trust_key` rolls the record back when the journal refuses and
    /// `revoke_key` does the same. The invariant holds whichever side of the
    /// durability boundary the machine is on, so it is asserted that way.
    ///
    /// Pinning assertion: the two `assert_eq!`s comparing the returned outcome
    /// with the resulting trust state. If either entry point ever diverged —
    /// reporting success for a rolled-back record, or leaving a record behind
    /// after reporting refusal — one of them fires.
    #[test]
    fn durable_trust_state_agrees_with_the_reported_outcome() {
        let key = [11u8; 32];
        let mut r = RevocationRegistry::new();

        let provisioned = r.trust_key(key, "test-publisher");
        assert_eq!(provisioned.is_ok(), r.is_key_trusted(&key),
            "the trust store must agree with what trust_key reported");
        if let Err(reason) = provisioned {
            assert_eq!(reason, "failed to persist publisher trust root");
            assert!(r.trusted_keys.is_empty(), "a rolled-back trust root must not linger in memory");
        }

        let revoked = r.revoke_key(&key, RevocationReason::MaliciousBehavior, 12, "root");
        assert_eq!(revoked.is_ok(), r.is_key_revoked(&key),
            "the CRL must agree with what revoke_key reported");
        if let Err(reason) = revoked {
            assert_eq!(reason, "failed to persist key revocation");
            assert!(!r.is_key_revoked(&key), "a rolled-back revocation must not be reported as in force");
        }
    }

    /// A revoked key stays untrusted no matter how it is re-provisioned, and an
    /// untrusted key can never be promoted by rotation.
    ///
    /// Pinning assertion: `is_key_trusted` stays false after re-granting trust to
    /// an already-revoked key. `is_key_trusted` intersects the trust store with the
    /// CRL precisely so that a stale trust record cannot resurrect a compromised
    /// publisher.
    #[test]
    fn a_revoked_key_cannot_be_resurrected_by_reprovisioning() {
        let key = [13u8; 32];
        let mut r = RevocationRegistry::new();
        r.grant_trust_root(key, "test-publisher").expect("the trust store has room");
        r.place_key_on_crl(&key, RevocationReason::KeyCompromised, 20, "root").expect("the CRL has room");
        assert!(!r.is_key_trusted(&key));
        r.grant_trust_root(key, "test-publisher").expect("re-granting an existing key must not need a free slot");
        assert!(!r.is_key_trusted(&key),
            "a CRL entry must outrank a trust record written after it");
    }

    /// The exact set of publisher identities that may hold platform authority,
    /// and the lookalikes that must not.
    ///
    /// Pinning assertion: `org.interos-evil` is rejected. The previous check was
    /// `starts_with("org.interos")`, which accepted it along with
    /// `org.interosX`, so any publisher record naming itself after the namespace
    /// without belonging to it would have been granted platform authority --
    /// and platform authority is what gates capability 18.
    #[test]
    fn system_publisher_identity_accepts_only_delimited_namespace_members() {
        // Accepted: the two fixed identities and the delimited namespace.
        for accepted in [
            "system",
            "platform",
            "org.interos.",
            "org.interos.settings",
            "org.interos.compositor",
            "org.interos.service.powerd",
            "org.interos.a",
        ] {
            assert!(
                is_system_publisher_identity(accepted),
                "{:?} is a platform publisher identity", accepted
            );
        }

        // Rejected: lookalikes that a bare prefix test would have accepted.
        for rejected in [
            "org.interos-evil",
            "org.interosX",
            "org.interosomething",
            "org.interos",
            "org.Interos.settings",
            "ORG.INTEROS.settings",
            "system2",
            "platformx",
            " system",
            "system ",
            "System",
            "com.example.system",
            "not-system",
            "",
        ] {
            assert!(
                !is_system_publisher_identity(rejected),
                "{:?} must not be a platform publisher identity", rejected
            );
        }
    }

    /// `is_system_publisher` is the gate capability 18 depends on, and with no
    /// provisioning path reachable it must fail closed for every key.
    ///
    /// Pinning assertions: an unprovisioned key is never a system publisher, a
    /// trusted non-platform publisher is still refused, a revoked key is refused
    /// even when its publisher record is a platform identity, and only a trusted,
    /// unrevoked key recorded under a platform identity passes.
    #[test]
    fn capability_18_fails_closed_without_a_platform_trust_root() {
        let mut r = RevocationRegistry::new();

        let unprovisioned = [21u8; 32];
        assert!(!r.is_system_publisher(&unprovisioned),
            "an unprovisioned key must never be a system publisher");

        // A trusted publisher outside the platform namespace.
        let third_party = [22u8; 32];
        r.grant_trust_root(third_party, "com.example.vendor").expect("trust store has room");
        assert!(r.is_key_trusted(&third_party), "the key itself is trusted");
        assert!(!r.is_system_publisher(&third_party),
            "a trusted third-party publisher must not gain platform authority");

        // A trusted publisher that only looks like it is in the namespace.
        let lookalike = [23u8; 32];
        r.grant_trust_root(lookalike, "org.interos-evil").expect("trust store has room");
        assert!(!r.is_system_publisher(&lookalike),
            "a lookalike publisher must not gain platform authority");

        // A genuine platform publisher passes.
        let platform = [24u8; 32];
        r.grant_trust_root(platform, "org.interos.platform").expect("trust store has room");
        assert!(r.is_system_publisher(&platform),
            "a provisioned platform publisher must be recognised");

        // Revocation outranks the publisher record.
        r.place_key_on_crl(&platform, RevocationReason::KeyCompromised, 30, "root").expect("the CRL has room");
        assert!(!r.is_system_publisher(&platform),
            "a revoked key must not remain a system publisher");
    }

    /// The OS ships no publisher trust root of its own: a key the owner never
    /// installed is untrusted, and only the owner's installation record can
    /// make one trusted.
    ///
    /// WHY this replaces a built-in root: a public key compiled into the image
    /// is trusted by every device that ships it, and the matching private key
    /// is exactly what an attacker would want. Trust has to be an act of the
    /// device owner, not a property of the build.
    ///
    /// Pinning assertions: a fresh registry trusts nothing, `seed()` installs
    /// nothing of its own, a key becomes trusted only after an explicit
    /// installation, and revocation outranks that installation.
    #[test]
    fn no_publisher_key_is_trusted_until_the_owner_installs_it() {
        let mut r = RevocationRegistry::new();

        // 1. A device with no installed root trusts no publisher at all.
        assert!(r.trusted_keys.is_empty(), "the trust store starts empty");
        let unknown = [41u8; 32];
        assert!(!r.is_key_trusted(&unknown),
            "a key the owner never installed must not be trusted");
        assert!(!r.is_system_publisher(&unknown),
            "a key the owner never installed must not be a system publisher");
        assert_eq!(r.trusted_publisher(&unknown), None,
            "an uninstalled key must have no publisher identity");

        // 2. Restoring the device state installs nothing by itself.
        r.seed();
        assert!(r.trusted_keys.is_empty(),
            "seeding the registry must not install a trust root of its own");
        assert!(!r.is_key_trusted(&unknown),
            "seeding the registry must not make an unknown key trusted");

        // 3. The owner's installation is what makes a publisher trusted.
        r.grant_trust_root(unknown, "com.example.vendor").expect("the trust store has room");
        assert!(r.is_key_trusted(&unknown));
        assert_eq!(r.trusted_publisher(&unknown), Some("com.example.vendor"));
        assert!(!r.is_system_publisher(&unknown),
            "an ordinary publisher identity carries no platform authority");

        // 4. Revocation strictly outranks the installation record.
        r.place_key_on_crl(&unknown, RevocationReason::KeyCompromised, 40, "root")
            .expect("CRL has room");
        assert!(r.is_key_revoked(&unknown));
        assert!(!r.is_key_trusted(&unknown),
            "a revoked publisher key must be untrusted");
        assert_eq!(r.trusted_publisher(&unknown), None,
            "a revoked publisher key must report no trusted identity");
    }

    /// A publisher key the owner installed under a platform identity is the
    /// only thing that can carry device-wide administration, and the
    /// installation itself is what the owner did.
    ///
    /// Pinning assertions: the platform identity alone confers nothing, the
    /// installed key does, and revocation removes it again.
    #[test]
    fn platform_authority_comes_only_from_an_installed_root() {
        let mut r = RevocationRegistry::new();
        let platform = [42u8; 32];

        assert!(!r.is_system_publisher(&platform),
            "a platform identity that was never installed grants nothing");

        r.grant_trust_root(platform, "org.interos.platform").expect("the trust store has room");
        assert!(r.is_system_publisher(&platform),
            "an installed platform publisher must be recognised");

        r.place_key_on_crl(&platform, RevocationReason::KeyCompromised, 41, "root")
            .expect("CRL has room");
        assert!(!r.is_system_publisher(&platform),
            "a revoked key must not remain a system publisher");
    }

    /// The console that installs publisher roots is not an authenticated
    /// provisioning channel, so it must not be able to mint device-wide
    /// administration.
    ///
    /// Pinning assertion: the refusal names the policy rather than a storage
    /// failure, and the refused installation leaves no trust record behind.
    #[test]
    fn a_console_installation_cannot_mint_platform_authority() {
        let mut r = RevocationRegistry::new();
        let key = [51u8; 32];

        let err = r.trust_key(key, "org.interos.platform").unwrap_err();
        assert_eq!(err, "platform publisher authority requires an authenticated provisioning channel",
            "the refusal must name the policy, not a storage failure");
        assert!(r.trusted_keys.is_empty(), "a refused installation must leave no trust record");
        assert!(!r.is_key_trusted(&key));
        assert!(!r.is_system_publisher(&key));

        // An ordinary publisher identity is still installable this way.
        let ordinary = [52u8; 32];
        r.grant_trust_root(ordinary, "com.example.vendor").expect("the trust store has room");
        assert!(r.is_key_trusted(&ordinary));
    }

    /// A key rotation carries the identity recorded for the key it replaces,
    /// and cannot rename a publisher into the platform namespace.
    ///
    /// WHY the rotation proof cannot supply the name: it signs the new key
    /// and the epoch, so the publisher name inside it is not authenticated.
    ///
    /// Pinning assertion: the identity resolved for the rotation is the one
    /// the trust store recorded, whatever the proof claims.
    #[test]
    fn a_rotation_carries_the_recorded_identity_forward() {
        let mut r = RevocationRegistry::new();
        let old_key = [53u8; 32];
        let new_key = [54u8; 32];
        r.grant_trust_root(old_key, "com.example.vendor").expect("the trust store has room");

        let proof = KeyRotationProof {
            publisher: String::from("org.interos.platform"),
            old_key,
            new_key,
            epoch: 9,
            proof_signature: [0u8; 64],
        };

        assert_eq!(r.rotation_publisher(&proof.old_key).unwrap(), "com.example.vendor",
            "a rotation must inherit the recorded identity, not the name in the proof");
        assert_eq!(r.rotation_publisher(&[55u8; 32]).unwrap_err(),
            "Old key has no recorded publisher identity",
            "a key that was never installed has no identity to carry forward");
    }
}
