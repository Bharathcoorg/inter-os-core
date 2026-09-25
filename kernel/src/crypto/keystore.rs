//! Hardware-Isolated Secure Enclave & Keystore Subsystem
//!
//! Provides cryptographic Key Derivation (KDF) using RFC 4231 HMAC-SHA256 and TRNG entropy,
//! hardware key slot isolation, and authenticated encrypted credential storage
//! with zero-trust tamper verification.

use alloc::vec::Vec;
use spin::Mutex;
use crate::crypto::hmac::HmacSha256;
use crate::crypto::get_random_bytes;

/// Maximum number of hardware key slots.
pub const MAX_KEY_SLOTS: usize = 16;

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

/// Hardware-Isolated Secure Keystore
pub struct SecureKeystore {
    master_seed: [u8; 32],
    entries: Vec<KeystoreEntry>,
}

impl SecureKeystore {
    pub const fn new() -> Self {
        Self {
            master_seed: [0u8; 32],
            entries: Vec::new(),
        }
    }

    /// Derives a 32-byte cryptographic key for a specific slot using domain-separated HMAC-SHA256.
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

        // Generate 16-byte cryptographic random nonce from hardware TRNG
        let mut nonce = [0u8; 16];
        get_random_bytes(&mut nonce);

        // Derive slot encryption key
        let slot_key = self.derive_key(slot, b"cred_storage");

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

        let entry = self
            .entries
            .iter()
            .find(|e| e.slot == slot_idx && &e.alias[..e.alias_len] == alias_bytes)
            .ok_or(KeystoreError::EntryNotFound)?;

        let slot_key = self.derive_key(slot, b"cred_storage");

        // 1. Verify authentication tag in constant time
        let mut auth_input = Vec::new();
        auth_input.extend_from_slice(b"AUTH_TAG_V1:");
        auth_input.extend_from_slice(alias_str.as_bytes());
        auth_input.extend_from_slice(&entry.nonce);
        auth_input.extend_from_slice(&entry.ciphertext);
        let expected_tag = HmacSha256::compute(&slot_key, &auth_input);

        if !constant_time_eq(&entry.auth_tag, &expected_tag) {
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
                block_idx += 1;
            }
            plaintext.push(byte ^ block_keystream[i % 32]);
        }

        Ok(plaintext)
    }
}

/// Constant-time byte array equality comparison preventing side-channel leaks.
#[inline(never)]
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

pub static SECURE_KEYSTORE: Mutex<SecureKeystore> = Mutex::new(SecureKeystore::new());

/// Initializes the Secure Keystore and verifies derivation & authenticated tamper-proofing.
pub fn init() {
    let mut ks = SECURE_KEYSTORE.lock();

    // The storage-encryption root is never a compiled-in constant. Persist the
    // first generated device seed in the journal and reload it on subsequent boots.
    let mut seed_record = [0u8; 40];
    let loaded = crate::storage::persistent::load_latest_kind(&mut seed_record, 6)
        .ok()
        .flatten();
    if let Some((_generation, len)) = loaded {
        if len == 40 && &seed_record[..4] == b"IKS1" {
            ks.master_seed.copy_from_slice(&seed_record[8..40]);
        }
    }
    if ks.master_seed == [0u8; 32] {
        crate::crypto::get_random_bytes(&mut ks.master_seed);
        let mut record = [0u8; 40];
        record[..4].copy_from_slice(b"IKS1");
        record[4..8].copy_from_slice(&1u32.to_le_bytes());
        record[8..40].copy_from_slice(&ks.master_seed);
        let _ = crate::storage::persistent::commit_kind(&record, 6);
    }

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

    // 2. Test authenticated encryption and storage
    let secret_payload = b"INTEROS_SECURE_AUTH_SESSION_TOKEN_9812";
    ks.store_credential(KeySlot::PaymentCredentialKey, "auth.session", secret_payload)
        .expect("Failed to store payment credential");

    // 3. Retrieve and verify plaintext equality
    let retrieved = ks
        .retrieve_credential(KeySlot::PaymentCredentialKey, "auth.session")
        .expect("Failed to retrieve payment credential");
    assert_eq!(retrieved.as_slice(), secret_payload);

    // 4. Test tamper detection: flip 1 bit in stored ciphertext
    if let Some(entry) = ks.entries.iter_mut().find(|e| &e.alias[..e.alias_len] == b"auth.session") {
        if !entry.ciphertext.is_empty() {
            entry.ciphertext[0] ^= 0x01; // Tamper 1 bit!
        }
    }
    let tamper_result = ks.retrieve_credential(KeySlot::PaymentCredentialKey, "auth.session");
    assert_eq!(
        tamper_result,
        Err(KeystoreError::IntegrityViolation),
        "Keystore must reject tampered ciphertext with IntegrityViolation"
    );

    // 5. Restore clean entry for production use
    ks.store_credential(KeySlot::PaymentCredentialKey, "auth.session", secret_payload)
        .expect("Failed to restore clean credential");

    // Pre-seed eSIM subscriber credential
    ks.store_credential(KeySlot::TelephonyAuthKey, "esim.zk_subscriber", b"SOV_SUB_AUTH_TOKEN_99")
        .expect("Failed to store eSIM subscriber credential");
}
