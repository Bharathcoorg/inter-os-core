// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Encrypted storage boundary for mutable filesystem payloads.
//!
//! Keeps filesystem structure usable by the kernel while encrypting mutable
//! file payloads at rest. The root key comes from the persistent secure keystore;
//! each blob receives a fresh nonce and HMAC-SHA256 authentication tag.

use alloc::vec::Vec;
use crate::crypto::{get_random_bytes, HmacSha256, KeySlot, SECURE_KEYSTORE, constant_time_eq_32};

pub const ENCRYPTED_MAGIC: [u8; 4] = *b"ENC1";
pub const ENCRYPTED_VERSION: u8 = 1;
pub const ENCRYPTED_HEADER_SIZE: usize = 26;
pub const ENCRYPTED_TAG_SIZE: usize = 32;
pub const ENCRYPTED_OVERHEAD: usize = ENCRYPTED_HEADER_SIZE + ENCRYPTED_TAG_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionError {
    InvalidFormat,
    BufferTooSmall,
    IntegrityViolation,
    TooLarge,
    /// No persistent storage root is established, so no key can be derived.
    NoStorageRoot,
}

/// The storage-at-rest root key.
///
/// WHY this is a `Result`: `SecureKeystore::derive_key` returns a key rather than
/// a result, so it cannot report that the root it is deriving from is absent.
/// Called ungated, this would compute `HMAC(0, ...)` — a fixed function of
/// published constants that anyone can reproduce — and encrypt every payload
/// under it. Gating here is the only place that refusal can happen, because both
/// the seal and the open path funnel through this function.
///
/// WHY the guard is `is_seeded()` and not "the seed is non-zero": `is_seeded`
/// is the keystore's own statement that a root was *established*, which is
/// exactly the guarantee a caller needs and is not the same question as whether
/// the current seed bytes happen to be non-zero.
fn storage_key() -> Result<[u8; 32], EncryptionError> {
    let ks = SECURE_KEYSTORE.lock();
    if !ks.is_seeded() {
        return Err(EncryptionError::NoStorageRoot);
    }
    Ok(ks.derive_key(KeySlot::SovereignMasterKey, b"storage-at-rest-v1"))
}

/// Encrypts a bounded payload into an authenticated ENC1 envelope.
pub fn seal(plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), EncryptionError> {
    if plaintext.len() > u32::MAX as usize { return Err(EncryptionError::TooLarge); }
    // Fail closed before any key material exists: producing a ciphertext here
    // would be a payload readable by anyone holding the zero seed.
    let key = storage_key()?;
    let mut nonce = [0u8; 16];
    get_random_bytes(&mut nonce);

    let mut ciphertext = Vec::with_capacity(plaintext.len());
    let mut stream = [0u8; 32];
    for (i, &byte) in plaintext.iter().enumerate() {
        if i % 32 == 0 {
            let block = (i / 32) as u32;
            let mut input = [0u8; 20];
            input[..16].copy_from_slice(&nonce);
            input[16..20].copy_from_slice(&block.to_be_bytes());
            stream = HmacSha256::compute(&key, &input);
        }
        ciphertext.push(byte ^ stream[i % 32]);
    }

    out.clear();
    out.reserve(ENCRYPTED_OVERHEAD + ciphertext.len());
    out.extend_from_slice(&ENCRYPTED_MAGIC);
    out.push(ENCRYPTED_VERSION);
    out.push(0);
    out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    let tag = HmacSha256::compute(&key, out);
    out.extend_from_slice(&tag);
    Ok(())
}

/// Authenticates and decrypts an ENC1 envelope.
pub fn open(envelope: &[u8], out: &mut [u8]) -> Result<usize, EncryptionError> {
    if envelope.len() < ENCRYPTED_OVERHEAD { return Err(EncryptionError::InvalidFormat); }
    if envelope[..4] != ENCRYPTED_MAGIC || envelope[4] != ENCRYPTED_VERSION {
        return Err(EncryptionError::InvalidFormat);
    }
    let plain_len = u32::from_le_bytes([envelope[6],envelope[7],envelope[8],envelope[9]]) as usize;
    let expected_len = ENCRYPTED_OVERHEAD.checked_add(plain_len).ok_or(EncryptionError::InvalidFormat)?;
    if expected_len != envelope.len() { return Err(EncryptionError::InvalidFormat); }
    if out.len() < plain_len { return Err(EncryptionError::BufferTooSmall); }

    let tag_offset = envelope.len() - ENCRYPTED_TAG_SIZE;
    let mut expected_tag = [0u8; 32];
    expected_tag.copy_from_slice(&envelope[tag_offset..]);
    // Same refusal as on the write path. Checking the tag under a key derived
    // from an absent root would compare the caller's tag against one anyone can
    // compute, so the "authenticated" verdict would mean nothing.
    let key = storage_key()?;
    let actual_tag = HmacSha256::compute(&key, &envelope[..tag_offset]);
    if !constant_time_eq_32(&actual_tag, &expected_tag) {
        return Err(EncryptionError::IntegrityViolation);
    }

    let nonce = &envelope[10..26];
    let ciphertext = &envelope[26..tag_offset];
    let mut stream = [0u8; 32];
    for i in 0..plain_len {
        if i % 32 == 0 {
            let block = (i / 32) as u32;
            let mut input = [0u8; 20];
            input[..16].copy_from_slice(nonce);
            input[16..20].copy_from_slice(&block.to_be_bytes());
            stream = HmacSha256::compute(&key, &input);
        }
        out[i] = ciphertext[i] ^ stream[i % 32];
    }
    Ok(plain_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no storage root established, `derive_key` still returns a key — it
    /// is `HMAC(0, ...)`, which anyone can compute. So an unguarded caller
    /// encrypts under published constants and "verifies" against a tag the
    /// reader can also produce, and neither operation reports anything wrong.
    ///
    /// Both entry points must refuse instead. This is the assertion that proves
    /// the guard is in the shared key function rather than in one caller.
    ///
    /// WHY the precondition is asserted rather than assumed: the result differs
    /// depending on whether a root is established, and no test establishes one, so
    /// this must fail loudly rather than silently check the other branch if that
    /// ever changes.
    #[test]
    fn both_directions_refuse_when_no_storage_root_is_established() {
        assert!(
            !SECURE_KEYSTORE.lock().is_seeded(),
            "no unit test establishes a storage root; this test's branch depends on there being none",
        );

        let mut sealed = Vec::new();
        assert_eq!(
            seal(b"interos at rest", &mut sealed),
            Err(EncryptionError::NoStorageRoot),
            "sealing without a root must refuse rather than derive from an absent seed",
        );
        assert!(
            sealed.is_empty(),
            "a refused seal must not leave a ciphertext behind that something could adopt",
        );

        // A well-formed envelope: the refusal is about the missing key, not about
        // the input, so the key gate has to be the thing that fires.
        let mut envelope = Vec::new();
        envelope.extend_from_slice(&ENCRYPTED_MAGIC);
        envelope.push(ENCRYPTED_VERSION);
        envelope.push(0);
        envelope.extend_from_slice(&4u32.to_le_bytes());
        envelope.extend_from_slice(&[0u8; 16]);
        envelope.extend_from_slice(&[0u8; 4]);
        envelope.extend_from_slice(&[0u8; 32]);
        let mut out = [0u8; 4];
        assert_eq!(
            open(&envelope, &mut out),
            Err(EncryptionError::NoStorageRoot),
            "opening without a root must refuse rather than check the tag under a derivable key",
        );
    }
}
