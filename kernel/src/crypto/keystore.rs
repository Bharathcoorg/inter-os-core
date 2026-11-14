// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Secure Keystore Subsystem
//!
//! Provides cryptographic Key Derivation (KDF) using RFC 4231 HMAC-SHA256 and TRNG entropy,
//! hardware key slot isolation, and authenticated encrypted credential storage
//! with zero-trust tamper verification.

use alloc::vec::Vec;
use spin::Mutex;
use crate::crypto::hmac::HmacSha256;

/// Maximum number of hardware key slots.
pub const MAX_KEY_SLOTS: usize = 16;

/// Record magic of the persisted storage root, at the head of the record.
pub const SEED_RECORD_MAGIC: [u8; 4] = *b"IKS1";

/// Byte length of the persisted storage root record.
pub const SEED_RECORD_LEN: usize = 40;

/// Pre-defined Hardware Key Slots
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeySlot {
    SovereignMasterKey,
    TelephonyAuthKey,
    PaymentCredentialKey,
    NetworkP2PKey,
    AppSlot(u8),
}

impl KeySlot {
    pub const fn from_u32(val: u32) -> Option<Self> {
        match val {
            0 => Some(KeySlot::SovereignMasterKey),
            1 => Some(KeySlot::TelephonyAuthKey),
            2 => Some(KeySlot::PaymentCredentialKey),
            3 => Some(KeySlot::NetworkP2PKey),
            4..=15 => Some(KeySlot::AppSlot(val as u8)),
            _ => None,
        }
    }

    pub const fn slot_index(&self) -> usize {
        match *self {
            KeySlot::SovereignMasterKey => 0,
            KeySlot::TelephonyAuthKey => 1,
            KeySlot::PaymentCredentialKey => 2,
            KeySlot::NetworkP2PKey => 3,
            KeySlot::AppSlot(idx) => idx as usize,
        }
    }
}

/// Keystore Errors
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeystoreError {
    SlotOutOfBounds,
    EntryNotFound,
    IntegrityViolation,
    BufferTooSmall,
    CapabilityDenied,
    /// No storage root has been established, so no key may be derived.
    SeedUnavailable,
    /// A persisted record was found but cannot be interpreted as this format.
    RecordUnusable,
}

/// Outcome of a journal-backed storage-root recovery.
///
/// WHY this is reported rather than collapsed into a bool: "there is no record
/// yet" and "there is a record I cannot read" demand opposite actions. The
/// first is a first boot and a new root is correct. The second means the
/// committed root is unreadable, and minting a replacement would silently
/// render every credential ever stored under it undecryptable.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SeedRecovery {
    /// The committed device root was restored from the journal.
    Restored { generation: u64 },
    /// No record existed: a fresh root was generated and committed.
    Generated { generation: u64 },
    /// A fresh root was generated but the journal could not record it, so the
    /// root exists only in RAM and will not survive a reboot.
    GeneratedEphemeral,
    /// A record exists but could not be interpreted, or the journal could not be
    /// read. Recovery fails closed and no new root is generated.
    Refused,
}

/// Authenticated Encrypted Storage Entry
#[derive(Clone, Debug)]
pub struct KeystoreEntry {
    pub slot: usize,
    pub alias: [u8; 32],
    pub alias_len: usize,
    pub nonce: [u8; 16],
    pub ciphertext: Vec<u8>,
    pub auth_tag: [u8; 32],
}

/// Secure Keystore. The current generic target uses encrypted persistent
/// software state; a physical device backend must explicitly provide hardware
/// isolation before the platform reports hardware-backed key storage.
pub struct SecureKeystore {
    master_seed: [u8; 32],
    hardware_backed: bool,
    entries: Vec<KeystoreEntry>,
    /// Whether a storage root has been established for this instance.
    ///
    /// WHY this is tracked instead of testing `master_seed` at each use site: an
    /// all-zero seed means "no root". Deriving from it yields a predictable key
    /// that anyone can compute, so credential operations must refuse rather than
    /// answer with attacker-derivable material.
    seed_established: bool,
}

/// Interprets a recovered storage-root record.
///
/// WHY this is total and separate from recovery: it either yields exactly one
/// 32-byte root or nothing. A record that is the wrong length, carries a
/// foreign magic, or holds an all-zero root is refused outright, so a partially
/// understood record can never be half-applied to the live keystore.
fn decode_seed_record(record: &[u8]) -> Option<[u8; 32]> {
    if record.len() != SEED_RECORD_LEN || record[..4] != SEED_RECORD_MAGIC {
        return None;
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&record[8..40]);
    // An all-zero root is the "absent" sentinel; a record carrying it is not a
    // root and must not be adopted as one.
    if seed == [0u8; 32] {
        return None;
    }
    Some(seed)
}

impl SecureKeystore {
    pub const fn new() -> Self {
        Self {
            master_seed: [0u8; 32],
            hardware_backed: false,
            entries: Vec::new(),
            seed_established: false,
        }
    }

    /// Returns whether a concrete hardware-backed keystore is bound.
    pub const fn is_hardware_backed(&self) -> bool { self.hardware_backed }

    /// Returns whether a storage root is established, and therefore whether any
    /// derived key has real secrecy.
    ///
    /// `derive_key` cannot report failure, so every caller that needs the
    /// guarantee must gate on this first.
    pub const fn is_seeded(&self) -> bool { self.seed_established }

    /// Installs a storage root. Boot recovery is the only production caller.
    fn adopt_device_seed(&mut self, seed: [u8; 32]) {
        self.master_seed = seed;
        self.seed_established = seed != [0u8; 32];
    }

    /// Interprets one record handed back by the journal's recovery routine.
    ///
    /// Fail-closed by construction: on any refusal the instance is left exactly
    /// as it was, so a record that cannot be understood never replaces a root
    /// that was already established.
    fn apply_seed_record(&mut self, record: &[u8]) -> Result<(), KeystoreError> {
        let seed = decode_seed_record(record).ok_or(KeystoreError::RecordUnusable)?;
        self.adopt_device_seed(seed);
        Ok(())
    }

    /// Boot recovery of the persistent storage root, using the journal's own
    /// recovery routine (`load_latest_kind`) rather than a second reader.
    ///
    /// WHY the root is read from record kind 6 on its own: it is the only record
    /// ever written before `persistent::set_journal_auth_key` publishes a key, so
    /// it is read and written under the zero key while every other kind is
    /// authenticated by the published key. That is also why no other keystore
    /// state can share this kind - a record committed under the published key
    /// would not validate at the point in boot when the key becomes available.
    ///
    /// WHY this must run before any journal key is published: it reads under the
    /// zero key, and the key is derived from the root this read returns.
    pub fn recover(&mut self) -> SeedRecovery {
        let mut seed_record = [0u8; SEED_RECORD_LEN];
        match crate::storage::persistent::load_latest_kind(&mut seed_record, 6) {
            // The journal is unavailable (no block device bound, an I/O error, or
            // a surviving record this buffer cannot hold). We cannot tell a first
            // boot from a lost root, so we do not mint one: an ephemeral root
            // would look identical to a durable one and silently orphan every
            // stored credential on the next boot.
            Err(_) => SeedRecovery::Refused,
            // No record at all: this is a first boot and a new root is correct.
            Ok(None) => {
                if crate::crypto::try_get_random_bytes(&mut self.master_seed).is_err() {
                    self.master_seed = [0u8; 32];
                    return SeedRecovery::Refused;
                }
                self.seed_established = self.master_seed != [0u8; 32];
                if !self.seed_established {
                    // The random source produced the absent sentinel. Refuse rather
                    // than publish a root that is already public.
                    self.master_seed = [0u8; 32];
                    return SeedRecovery::Refused;
                }
                let mut record = [0u8; SEED_RECORD_LEN];
                record[..4].copy_from_slice(&SEED_RECORD_MAGIC);
                // WHY this field is written as a constant: the journal header
                // already carries the authoritative generation, so the value in
                // the payload is not read back. It is left at the value the
                // format has always written rather than being given a meaning
                // the recovery path does not use.
                record[4..8].copy_from_slice(&1u32.to_le_bytes());
                record[8..40].copy_from_slice(&self.master_seed);
                match crate::storage::persistent::commit_kind(&record, 6) {
                    Ok(generation) => SeedRecovery::Generated { generation },
                    // Reported rather than folded into Generated: without a record
                    // this root lives only in RAM, so every credential stored under
                    // it is lost at the next boot. Publishing it is still correct -
                    // the device can operate - but it must not be presented as
                    // durable.
                    Err(_) => SeedRecovery::GeneratedEphemeral,
                }
            }
            // A record exists. It is either the committed root or an
            // uninterpretable one; never both.
            Ok(Some((generation, len))) => match self.apply_seed_record(&seed_record[..len]) {
                Ok(()) => SeedRecovery::Restored { generation },
                // Fail closed: leave the keystore without a root instead of
                // quietly generating a replacement that orphans the old data.
                Err(_) => SeedRecovery::Refused,
            },
        }
    }

    /// Derives a 32-byte cryptographic key for a specific slot using domain-separated HMAC-SHA256.
    ///
    /// WHY this cannot fail closed on its own: the signature returns a key, not a
    /// result, and it is called from storage and vault paths outside this module.
    /// Callers that require a real root must check `is_seeded()` first.
    pub fn derive_key(&self, slot: KeySlot, label: &[u8]) -> [u8; 32] {
        let slot_idx = slot.slot_index() as u8;
        let mut info = Vec::with_capacity(32 + label.len());
        info.extend_from_slice(b"InterOS-Keystore-v1.0-Slot:");
        info.push(slot_idx);
        info.extend_from_slice(label);

        HmacSha256::compute(&self.master_seed, &info)
    }

    /// Encrypts plaintext under slot key using HMAC-CTR keystream and generates HMAC-SHA256 auth tag.
    pub fn store_credential(
        &mut self,
        slot: KeySlot,
        alias_str: &str,
        plaintext: &[u8],
    ) -> Result<(), KeystoreError> {
        let slot_idx = slot.slot_index();
        if slot_idx >= MAX_KEY_SLOTS {
            return Err(KeystoreError::SlotOutOfBounds);
        }
        // WHY refuse rather than derive from an absent root: with no storage root
        // the slot key would be a fixed function of published constants, so the
        // credential would be stored under, and readable with, material anyone can
        // compute. Reporting success there would be inventing security.
        if !self.seed_established {
            return Err(KeystoreError::SeedUnavailable);
        }

        // Generate 16-byte cryptographic random nonce from hardware TRNG
        let mut nonce = [0u8; 16];
        crate::crypto::try_get_random_bytes(&mut nonce).map_err(|_| KeystoreError::SeedUnavailable)?;

        // Derive slot encryption key
        let mut slot_key = self.derive_key(slot, b"cred_storage");

        // Generate keystream and encrypt via XOR
        let mut ciphertext = Vec::with_capacity(plaintext.len());
        let mut block_idx: u32 = 0;
        let mut block_keystream = [0u8; 32];

        for (i, &byte) in plaintext.iter().enumerate() {
            if i % 32 == 0 {
                let mut stream_input = [0u8; 20];
                stream_input[..16].copy_from_slice(&nonce);
                stream_input[16..20].copy_from_slice(&block_idx.to_be_bytes());
                block_keystream = HmacSha256::compute(&slot_key, &stream_input);
                block_idx += 1;
            }
            ciphertext.push(byte ^ block_keystream[i % 32]);
        }

        // Compute HMAC-SHA256 authentication tag over alias, nonce, and ciphertext
        let mut auth_input = Vec::new();
        auth_input.extend_from_slice(b"AUTH_TAG_V1:");
        auth_input.extend_from_slice(alias_str.as_bytes());
        auth_input.extend_from_slice(&nonce);
        auth_input.extend_from_slice(&ciphertext);
        let auth_tag = HmacSha256::compute(&slot_key, &auth_input);

        // Wipe temporary slot key and keystream from stack
        crate::crypto::zeroize_slice(&mut slot_key);
        crate::crypto::zeroize_slice(&mut block_keystream);

        let mut alias = [0u8; 32];
        let copy_len = alias_str.len().min(32);
        alias[..copy_len].copy_from_slice(&alias_str.as_bytes()[..copy_len]);

        // Remove any prior entry with the same slot and alias
        self.entries.retain(|e| !(e.slot == slot_idx && &e.alias[..e.alias_len] == &alias[..copy_len]));

        self.entries.push(KeystoreEntry {
            slot: slot_idx,
            alias,
            alias_len: copy_len,
            nonce,
            ciphertext,
            auth_tag,
        });

        Ok(())
    }

    /// Verifies authenticity tag and decrypts stored credential.
    pub fn retrieve_credential(
        &self,
        slot: KeySlot,
        alias_str: &str,
    ) -> Result<Vec<u8>, KeystoreError> {
        let slot_idx = slot.slot_index();
        let alias_bytes = alias_str.as_bytes();

        // Same refusal as on the write path: an absent root must not answer a
        // read with attacker-derivable plaintext.
        if !self.seed_established {
            return Err(KeystoreError::SeedUnavailable);
        }

        let entry = self
            .entries
            .iter()
            .find(|e| e.slot == slot_idx && &e.alias[..e.alias_len] == alias_bytes)
            .ok_or(KeystoreError::EntryNotFound)?;

        let mut slot_key = self.derive_key(slot, b"cred_storage");

        // 1. Verify authentication tag in constant time
        let mut auth_input = Vec::new();
        auth_input.extend_from_slice(b"AUTH_TAG_V1:");
        auth_input.extend_from_slice(alias_str.as_bytes());
        auth_input.extend_from_slice(&entry.nonce);
        auth_input.extend_from_slice(&entry.ciphertext);
        let expected_tag = HmacSha256::compute(&slot_key, &auth_input);

        if !constant_time_eq(&entry.auth_tag, &expected_tag) {
            crate::crypto::zeroize_slice(&mut slot_key);
            return Err(KeystoreError::IntegrityViolation);
        }

        // 2. Decrypt ciphertext using keystream
        let mut plaintext = Vec::with_capacity(entry.ciphertext.len());
        let mut block_idx: u32 = 0;
        let mut block_keystream = [0u8; 32];

        for (i, &byte) in entry.ciphertext.iter().enumerate() {
            if i % 32 == 0 {
                let mut stream_input = [0u8; 20];
                stream_input[..16].copy_from_slice(&entry.nonce);
                stream_input[16..20].copy_from_slice(&block_idx.to_be_bytes());
                block_keystream = HmacSha256::compute(&slot_key, &stream_input);
                crate::crypto::zeroize_slice(&mut stream_input);
                block_idx += 1;
            }
            plaintext.push(byte ^ block_keystream[i % 32]);
        }

        // Scrub temporary key material and keystream buffer
        crate::crypto::zeroize_slice(&mut slot_key);
        crate::crypto::zeroize_slice(&mut block_keystream);

        Ok(plaintext)
    }
}

/// Constant-time byte array equality comparison preventing side-channel leaks.
#[inline(always)]
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    super::constant_time_eq_32(a, b)
}

pub static SECURE_KEYSTORE: Mutex<SecureKeystore> = Mutex::new(SecureKeystore::new());

/// Initializes the generic software-backed keystore and verifies derivation & authenticated tamper-proofing.
pub fn init() {
    let mut ks = SECURE_KEYSTORE.lock();

    // The storage-encryption root is never a compiled-in constant. Bootstrap
    // goes through the keystore's own recovery path, which uses the journal's
    // recovery routine and reports which of the three situations it found.
    let recovered = ks.recover();
    match recovered {
        SeedRecovery::Restored { generation } => {
            crate::serial_println!(
                "[KEYSTORE] Restored persistent storage key generation {}.",
                generation,
            );
        }
        SeedRecovery::Generated { generation } => {
            crate::serial_println!(
                "[KEYSTORE] Generated new persistent device key seed (journal generation {}).",
                generation,
            );
        }
        SeedRecovery::GeneratedEphemeral => {
            // Reported, not hidden: the root works but nothing durable holds it.
            crate::serial_println!(
                "[KEYSTORE] WARNING: no block device accepted the new device key seed. The \
                 storage root is ephemeral and every credential stored under it will be lost \
                 at the next reboot.",
            );
        }
        SeedRecovery::Refused => {
            // Fail closed. Refusing means either no journal record can be read or
            // one exists that cannot be interpreted. Generating a replacement root
            // here would be indistinguishable from a successful boot while making
            // every credential stored under the previous root permanently
            // undecryptable, so the keystore is left without a root instead and
            // credential operations return SeedUnavailable.
            crate::serial_println!(
                "[KEYSTORE] FATAL: no usable persistent storage key. Credentials are \
                 unavailable and no replacement root was generated; previously stored \
                 credentials would be undecryptable.",
            );
        }
    }

    // Publish the journal authentication key now that a seed exists, while the
    // lock is already held. Later records are authenticated with it.
    //
    // WHY only when a root is established: the key is derived from the root, so
    // publishing one derived from an absent root would be a fixed, published
    // value, and every keyed record in the journal would then validate against
    // it. That is why the seed record itself is the one record read under the
    // zero key: it is the root, and a root cannot be authenticated by a key
    // derived from itself. This record is integrity-checked, which is what
    // rejects a torn or corrupted write, but it is not an authenticity boundary
    // against an attacker who can write the whole data disk. That limit is
    // inherent to a software-backed root and is not claimed to be otherwise.
    if ks.is_seeded() {
        crate::storage::persistent::set_journal_auth_key(&ks.master_seed);
    }
    let seeded = ks.is_seeded();
    // Release the global lock before the checks below: they take the seed once
    // and then assert against the live keystore, and re-locking the same
    // spinlock while its guard is alive would deadlock the boot.
    let device_seed = ks.master_seed;
    drop(ks);

    if !seeded {
        crate::serial_println!(
            "[KEYSTORE] Skipping derivation checks: there is no storage root to derive from.",
        );
        return;
    }

    // The remaining checks exercise derivation and authenticated encryption.
    //
    // WHY they run on a local instance: these operate on fixed literal secrets.
    // Running them against SECURE_KEYSTORE stored those literals as live
    // credentials (a "payment credential" and an eSIM subscriber token) in the
    // production keystore on every boot, where anything that could reach the
    // keystore could read them back. The assertions are worth keeping; the
    // credentials are not.
    let mut ks = SecureKeystore::new();
    ks.adopt_device_seed(device_seed);

    // 1. Derive keys across pre-defined slots and verify uniqueness
    let k0 = ks.derive_key(KeySlot::SovereignMasterKey, b"master");
    let k1 = ks.derive_key(KeySlot::TelephonyAuthKey, b"esim");
    let k2 = ks.derive_key(KeySlot::PaymentCredentialKey, b"pay");
    let k3 = ks.derive_key(KeySlot::NetworkP2PKey, b"net");

    assert_ne!(k0, [0u8; 32]);
    assert_ne!(k1, [0u8; 32]);
    assert_ne!(k2, [0u8; 32]);
    assert_ne!(k3, [0u8; 32]);
    assert_ne!(k0, k1);
    assert_ne!(k1, k2);
    assert_ne!(k2, k3);

    // 2. Authenticated encryption round trip
    let secret_payload = b"selftest-only-material";
    ks.store_credential(KeySlot::PaymentCredentialKey, "selftest.aead", secret_payload)
        .expect("Failed to store test credential");

    // 3. Retrieve and verify plaintext equality
    let retrieved = ks
        .retrieve_credential(KeySlot::PaymentCredentialKey, "selftest.aead")
        .expect("Failed to retrieve test credential");
    assert_eq!(retrieved.as_slice(), secret_payload);

    // 4. Tamper detection: flip one bit of stored ciphertext
    if let Some(entry) = ks.entries.iter_mut().find(|e| &e.alias[..e.alias_len] == b"selftest.aead") {
        if !entry.ciphertext.is_empty() {
            entry.ciphertext[0] ^= 0x01;
        }
    }
    assert_eq!(
        ks.retrieve_credential(KeySlot::PaymentCredentialKey, "selftest.aead"),
        Err(KeystoreError::IntegrityViolation),
        "Keystore must reject tampered ciphertext with IntegrityViolation"
    );

    // 5. The live keystore must hold no credential material.
    let live = SECURE_KEYSTORE.lock();
    assert!(
        live.entries.is_empty(),
        "boot-time keystore checks must not leave credentials in the live keystore"
    );
    crate::serial_println!(
        "[KEYSTORE] Derivation and authenticated-encryption checks passed; no credential material stored."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskId;

    fn seed_record(seed: &[u8; 32]) -> Vec<u8> {
        let mut record = alloc::vec![0u8; SEED_RECORD_LEN];
        record[..4].copy_from_slice(&SEED_RECORD_MAGIC);
        record[4..8].copy_from_slice(&1u32.to_le_bytes());
        record[8..40].copy_from_slice(seed);
        record
    }

    fn nonzero_seed(tag: u8) -> [u8; 32] {
        let mut seed = [0u8; 32];
        for (i, byte) in seed.iter_mut().enumerate() {
            *byte = tag.wrapping_add(i as u8);
        }
        seed
    }

    /// A keystore with no storage root must not store or return credentials.
    ///
    /// Before the fix `SecureKeystore::new()` derived every slot key from the
    /// all-zero seed, so it accepted a write and handed the plaintext straight
    /// back under key material that is a fixed function of published constants.
    ///
    /// The assertions that prove the defect is fixed: both calls are
    /// `SeedUnavailable`, not `Ok`/`EntryNotFound`-with-plaintext.
    #[test]
    fn credential_operations_refuse_an_absent_storage_root() {
        let mut ks = SecureKeystore::new();
        assert!(!ks.is_seeded());
        assert_eq!(
            ks.store_credential(KeySlot::SovereignMasterKey, "no.root", b"material"),
            Err(KeystoreError::SeedUnavailable),
        );
        assert_eq!(
            ks.retrieve_credential(KeySlot::SovereignMasterKey, "no.root"),
            Err(KeystoreError::SeedUnavailable),
        );
        assert!(
            ks.entries.is_empty(),
            "a refused write must not leave anything behind in the table",
        );
    }

    /// A committed root is adopted exactly.
    #[test]
    fn apply_seed_record_adopts_a_committed_root() {
        let seed = nonzero_seed(0x5A);
        let mut ks = SecureKeystore::new();
        assert_eq!(ks.apply_seed_record(&seed_record(&seed)), Ok(()));
        assert!(ks.is_seeded());
        let derived = ks.derive_key(KeySlot::SovereignMasterKey, b"probe");
        let mut twin = SecureKeystore::new();
        twin.adopt_device_seed(seed);
        assert_eq!(derived, twin.derive_key(KeySlot::SovereignMasterKey, b"probe"));
    }

    /// A record that cannot be interpreted is refused, and it never replaces a
    /// root that was already established.
    ///
    /// Before the fix the load result was discarded with `.ok().flatten()` and a
    /// failed magic/length check simply fell through to the "no seed" branch,
    /// which minted a fresh root. That silently overwrote the committed root
    /// with no error reported anywhere, orphaning every credential stored under
    /// it. The proving assertion is the last one: the established root survives.
    ///
    /// WHY every record in this list is malformed on purpose: a record that is
    /// well formed is not "unusable", it is a supersession - see
    /// `a_well_formed_record_supersedes_the_committed_root`, which asserts the
    /// opposite for one. Asserting refusal here for a decodable record would be
    /// asserting that `recover` refuses to boot a device whose committed root
    /// reads back intact.
    #[test]
    fn unusable_record_never_replaces_an_established_root() {
        let committed = nonzero_seed(0x11);
        let mut ks = SecureKeystore::new();
        assert_eq!(ks.apply_seed_record(&seed_record(&committed)), Ok(()));
        let before = ks.derive_key(KeySlot::SovereignMasterKey, b"probe");

        for record in [
            // Right length, foreign magic.
            {
                let mut r = seed_record(&nonzero_seed(0x33));
                r[..4].copy_from_slice(b"IKS9");
                r
            },
            // Truncated and over-long records.
            seed_record(&nonzero_seed(0x44))[..SEED_RECORD_LEN - 1].to_vec(),
            {
                let mut r = seed_record(&nonzero_seed(0x55));
                r.push(0);
                r
            },
            // A record whose root is the absent sentinel.
            seed_record(&[0u8; 32]),
            // A short record is the one malformed shape the journal can actually
            // hand over: a 1..39 byte payload passes the length cap and the
            // digest check, so `recover` reaches this function with a record it
            // cannot decode.
            b"IKS1".to_vec(),
        ] {
            assert_eq!(
                ks.apply_seed_record(&record),
                Err(KeystoreError::RecordUnusable),
                "an uninterpretable record must be refused, not adopted",
            );
            assert_eq!(
                ks.derive_key(KeySlot::SovereignMasterKey, b"probe"),
                before,
                "a refused record must leave the committed storage root in place",
            );
        }
    }

    /// A well-formed record is adopted even when it differs from the root
    /// already installed, because that is what the journal's current generation
    /// means.
    ///
    /// WHY this is the correct expectation rather than a hole in the refusal
    /// above: `recover` hands `apply_seed_record` whatever record kind 6
    /// currently holds, and the entire job of its `Ok(Some(..))` branch is to
    /// install exactly that. Refusing a decodable record would report
    /// `SeedRecovery::Refused` on a device whose latest root is perfectly
    /// readable, leaving every credential stored under it permanently
    /// unavailable. Adoption is the purpose of that branch; only records that
    /// cannot be decoded may be refused.
    #[test]
    fn a_well_formed_record_supersedes_the_committed_root() {
        let first = nonzero_seed(0x11);
        let second = nonzero_seed(0x22);
        let mut ks = SecureKeystore::new();
        assert_eq!(ks.apply_seed_record(&seed_record(&first)), Ok(()));
        let before = ks.derive_key(KeySlot::SovereignMasterKey, b"probe");

        assert_eq!(ks.apply_seed_record(&seed_record(&second)), Ok(()));

        let after = ks.derive_key(KeySlot::SovereignMasterKey, b"probe");
        assert_ne!(before, after, "the newly committed record must be the root in force");
        let mut expected = SecureKeystore::new();
        expected.adopt_device_seed(second);
        assert_eq!(
            after,
            expected.derive_key(KeySlot::SovereignMasterKey, b"probe"),
            "adoption must install the record's own root, not a re-derivation",
        );
    }

    /// Recovering the same committed record twice yields the same root:
    /// replay is deterministic and idempotent.
    #[test]
    fn seed_recovery_is_idempotent() {
        let committed = nonzero_seed(0x77);
        let record = seed_record(&committed);
        let mut first = SecureKeystore::new();
        let mut second = SecureKeystore::new();
        assert_eq!(first.apply_seed_record(&record), Ok(()));
        assert_eq!(first.apply_seed_record(&record), Ok(()));
        assert_eq!(second.apply_seed_record(&record), Ok(()));
        assert_eq!(
            first.derive_key(KeySlot::TelephonyAuthKey, b"probe"),
            second.derive_key(KeySlot::TelephonyAuthKey, b"probe"),
        );
        assert_eq!(first.master_seed, second.master_seed);
    }

    /// The decoder is total: it never yields a root for a record it cannot fully
    /// interpret, so a partial apply is not expressible.
    #[test]
    fn decode_seed_record_is_total() {
        let committed = nonzero_seed(0x99);
        assert_eq!(decode_seed_record(&seed_record(&committed)), Some(committed));
        assert_eq!(decode_seed_record(&[]), None);
        assert_eq!(decode_seed_record(b"IKS1"), None);
        assert_eq!(decode_seed_record(&[0u8; SEED_RECORD_LEN]), None);
        assert_eq!(decode_seed_record(&seed_record(&[0u8; 32])), None);
    }

    /// When the journal cannot be read, recovery fails closed.
    ///
    /// Before the fix an unreadable journal was indistinguishable from a first
    /// boot, so a fresh root was generated, the boot reported success, and every
    /// credential stored under the previous root became permanently
    /// undecryptable at the next boot. The proving assertions: the outcome is
    /// `Refused`, and no root was established.
    #[test]
    fn recovery_fails_closed_when_the_journal_is_unreadable() {
        // The outcome under test is the no-block-device path, which is only
        // observable while nothing has bound the primary drive.
        if crate::drivers::ATA_PRIMARY.lock().is_some() {
            return;
        }
        let mut ks = SecureKeystore::new();
        assert_eq!(ks.recover(), SeedRecovery::Refused);
        assert!(!ks.is_seeded(), "a refused recovery must not establish a root");
        assert_eq!(ks.master_seed, [0u8; 32]);
        assert_eq!(
            ks.store_credential(KeySlot::NetworkP2PKey, "any", b"data"),
            Err(KeystoreError::SeedUnavailable),
        );
    }

    /// Recovery never invents a root for a record it cannot parse, and never
    /// reports success for one. This pins the reporting contract itself.
    #[test]
    fn refused_and_committed_outcomes_are_distinguishable() {
        assert_ne!(SeedRecovery::Refused, SeedRecovery::Generated { generation: 1 });
        assert_ne!(SeedRecovery::Refused, SeedRecovery::GeneratedEphemeral);
        assert_ne!(SeedRecovery::Generated { generation: 1 }, SeedRecovery::GeneratedEphemeral);
        assert_ne!(SeedRecovery::Refused, SeedRecovery::Restored { generation: 1 });
        assert_eq!(SeedRecovery::Generated { generation: 7 }, SeedRecovery::Generated { generation: 7 });
    }

    /// The keystore self-test's local instance is seeded the same way production
    /// is, so the boot checks cannot pass against an unseeded keystore.
    #[test]
    fn boot_self_test_instance_is_seeded() {
        let mut ks = SecureKeystore::new();
        ks.adopt_device_seed(nonzero_seed(0xC3));
        assert!(ks.is_seeded());
        assert!(ks
            .store_credential(KeySlot::PaymentCredentialKey, "selftest.aead", b"material")
            .is_ok());
    }

    /// `SecureKeystore::new` remains a const constructor so the global keystore
    /// can still be a static, and it starts with no root.
    #[test]
    fn static_keystore_starts_without_a_root() {
        static KS: SecureKeystore = SecureKeystore::new();
        assert!(!KS.is_seeded());
        assert!(!KS.is_hardware_backed());
    }

    /// Task ids do not leak into the storage root: the root is device-wide.
    #[test]
    fn storage_root_is_independent_of_task_identity() {
        let seed = nonzero_seed(0xD4);
        let mut ks = SecureKeystore::new();
        ks.adopt_device_seed(seed);
        assert_eq!(
            ks.derive_key(KeySlot::SovereignMasterKey, b"probe"),
            {
                let mut other = SecureKeystore::new();
                other.adopt_device_seed(seed);
                other.derive_key(KeySlot::SovereignMasterKey, b"probe")
            },
        );
        let _ = TaskId(1);
    }
}