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
}

fn storage_key() -> [u8; 32] {
    SECURE_KEYSTORE.lock().derive_key(KeySlot::SovereignMasterKey, b"storage-at-rest-v1")
}

/// Encrypts a bounded payload into an authenticated ENC1 envelope.
pub fn seal(plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), EncryptionError> {
    if plaintext.len() > u32::MAX as usize { return Err(EncryptionError::TooLarge); }
    let key = storage_key();
    let mut nonce = [0u8; 16];
    get_random_bytes(&mut nonce);

    let mut ciphertext = Vec::with_capacity(plaintext.len());
    for (i, &byte) in plaintext.iter().enumerate() {
        let block = (i / 32) as u32;
        let mut input = [0u8; 20];
        input[..16].copy_from_slice(&nonce);
        input[16..20].copy_from_slice(&block.to_be_bytes());
        let stream = HmacSha256::compute(&key, &input);
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
    let key = storage_key();
    let actual_tag = HmacSha256::compute(&key, &envelope[..tag_offset]);
    if !constant_time_eq_32(&actual_tag, &expected_tag) {
        return Err(EncryptionError::IntegrityViolation);
    }

    let nonce = &envelope[10..26];
    let ciphertext = &envelope[26..tag_offset];
    for i in 0..plain_len {
        let block = (i / 32) as u32;
        let mut input = [0u8; 20];
        input[..16].copy_from_slice(nonce);
        input[16..20].copy_from_slice(&block.to_be_bytes());
        let stream = HmacSha256::compute(&key, &input);
        out[i] = ciphertext[i] ^ stream[i % 32];
    }
    Ok(plain_len)
}
