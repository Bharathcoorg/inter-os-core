// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Encrypted storage boundary for mutable filesystem payloads.
//!
//! Keeps filesystem structure usable by the kernel while encrypting mutable
//! file payloads at rest. The root key comes from the persistent secure keystore;
//! each blob receives a fresh nonce and HMAC-SHA256 authentication tag.
//!
//! Version 2 envelope format establishes strict cryptographic domain separation:
//! - Encryption Subkey: K_enc = HMAC(K, "ENC1-ENCRYPT-KEY-V2")
//! - Authentication Subkey: K_mac = HMAC(K, "ENC1-AUTHENTICATE-KEY-V2")
//!
//! Backward compatibility with Version 1 is preserved for read/migration.
//! Authentication tag is strictly verified before any plaintext bytes are decrypted.
//! All sensitive key buffers and keystreams are scrubbed with zeroize_slice.

use alloc::vec::Vec;
use crate::crypto::{try_get_random_bytes, ChaCha20Poly1305, HmacSha256, KeySlot, SECURE_KEYSTORE, constant_time_eq_32, zeroize_slice};

pub const ENCRYPTED_MAGIC: [u8; 4] = *b"ENC1";
pub const ENCRYPTED_VERSION_V1: u8 = 1;
pub const ENCRYPTED_VERSION_V2: u8 = 2;
pub const ENCRYPTED_VERSION_V3: u8 = 3;
pub const ENCRYPTED_VERSION: u8 = ENCRYPTED_VERSION_V2;
pub const ENCRYPTED_HEADER_SIZE: usize = 26;
pub const ENCRYPTED_TAG_SIZE: usize = 32;
pub const ENCRYPTED_OVERHEAD: usize = ENCRYPTED_HEADER_SIZE + ENCRYPTED_TAG_SIZE;

pub const ENCRYPTED_HEADER_SIZE_V3: usize = 22; // 4 magic + 1 ver + 1 res + 4 len + 12 nonce
pub const ENCRYPTED_TAG_SIZE_V3: usize = 16;
pub const ENCRYPTED_OVERHEAD_V3: usize = ENCRYPTED_HEADER_SIZE_V3 + ENCRYPTED_TAG_SIZE_V3;

const DOMAIN_ENC_V2: &[u8] = b"ENC1-ENCRYPT-KEY-V2";
const DOMAIN_MAC_V2: &[u8] = b"ENC1-AUTHENTICATE-KEY-V2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionError {
    InvalidFormat,
    BufferTooSmall,
    IntegrityViolation,
    TooLarge,
    /// No persistent storage root is established, so no key can be derived.
    NoStorageRoot,
    /// Hardware entropy unavailable to generate secure nonce.
    InsufficientEntropy,
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

/// Encrypts a bounded payload into an authenticated ENC1 Version 2 envelope using an explicit key.
pub fn seal_with_key(plaintext: &[u8], key: &[u8; 32], out: &mut Vec<u8>) -> Result<(), EncryptionError> {
    if plaintext.len() > u32::MAX as usize { return Err(EncryptionError::TooLarge); }
    let mut nonce = [0u8; 16];
    try_get_random_bytes(&mut nonce).map_err(|_| EncryptionError::InsufficientEntropy)?;

    let mut k_enc = HmacSha256::compute(key, DOMAIN_ENC_V2);
    let mut k_mac = HmacSha256::compute(key, DOMAIN_MAC_V2);

    let mut ciphertext = Vec::with_capacity(plaintext.len());
    let mut stream = [0u8; 32];
    for (i, &byte) in plaintext.iter().enumerate() {
        if i % 32 == 0 {
            let block = (i / 32) as u32;
            let mut input = [0u8; 20];
            input[..16].copy_from_slice(&nonce);
            input[16..20].copy_from_slice(&block.to_be_bytes());
            stream = HmacSha256::compute(&k_enc, &input);
            zeroize_slice(&mut input);
        }
        ciphertext.push(byte ^ stream[i % 32]);
    }
    zeroize_slice(&mut k_enc);
    zeroize_slice(&mut stream);

    out.clear();
    out.reserve(ENCRYPTED_OVERHEAD + ciphertext.len());
    out.extend_from_slice(&ENCRYPTED_MAGIC);
    out.push(ENCRYPTED_VERSION_V2);
    out.push(0);
    out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    let mut tag = HmacSha256::compute(&k_mac, out);
    zeroize_slice(&mut k_mac);
    out.extend_from_slice(&tag);
    zeroize_slice(&mut tag);
    Ok(())
}

/// Encrypts a bounded payload into an authenticated ENC1 envelope.
pub fn seal(plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), EncryptionError> {
    // Fail closed before any key material exists: producing a ciphertext here
    // would be a payload readable by anyone holding the zero seed.
    let mut key = storage_key()?;
    let res = seal_with_key(plaintext, &key, out);
    zeroize_slice(&mut key);
    res
}

/// Encrypts a bounded payload into an authenticated ENC1 Version 3 (ChaCha20-Poly1305) envelope using an explicit key.
pub fn seal_v3_with_key(plaintext: &[u8], key: &[u8; 32], out: &mut Vec<u8>) -> Result<(), EncryptionError> {
    if plaintext.len() > u32::MAX as usize { return Err(EncryptionError::TooLarge); }
    let mut nonce = [0u8; 12];
    try_get_random_bytes(&mut nonce).map_err(|_| EncryptionError::InsufficientEntropy)?;

    out.clear();
    out.reserve(ENCRYPTED_OVERHEAD_V3 + plaintext.len());
    out.extend_from_slice(&ENCRYPTED_MAGIC);
    out.push(ENCRYPTED_VERSION_V3);
    out.push(0);
    out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
    out.extend_from_slice(&nonce);
    let header_len = out.len();
    out.extend_from_slice(plaintext);

    let mut tag = [0u8; 16];
    let (header, payload) = out.split_at_mut(header_len);
    ChaCha20Poly1305::seal(key, &nonce, header, payload, &mut tag)
        .map_err(|_| EncryptionError::IntegrityViolation)?;

    out.extend_from_slice(&tag);
    zeroize_slice(&mut tag);
    zeroize_slice(&mut nonce);
    Ok(())
}

/// Encrypts a bounded payload into an authenticated ENC1 Version 3 (ChaCha20-Poly1305) envelope using storage key.
pub fn seal_v3(plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), EncryptionError> {
    let mut key = storage_key()?;
    let res = seal_v3_with_key(plaintext, &key, out);
    zeroize_slice(&mut key);
    res
}

/// Authenticates and decrypts an ENC1 envelope (supports Version 3 AEAD, Version 2 and legacy Version 1) using an explicit key.
pub fn open_with_key(envelope: &[u8], key: &[u8; 32], out: &mut [u8]) -> Result<usize, EncryptionError> {
    if envelope.len() < ENCRYPTED_HEADER_SIZE_V3 { return Err(EncryptionError::InvalidFormat); }
    if envelope[..4] != ENCRYPTED_MAGIC {
        return Err(EncryptionError::InvalidFormat);
    }
    let version = envelope[4];
    if version == ENCRYPTED_VERSION_V3 {
        if envelope.len() < ENCRYPTED_OVERHEAD_V3 { return Err(EncryptionError::InvalidFormat); }
        let plain_len = u32::from_le_bytes([envelope[6], envelope[7], envelope[8], envelope[9]]) as usize;
        let expected_len = ENCRYPTED_OVERHEAD_V3.checked_add(plain_len).ok_or(EncryptionError::InvalidFormat)?;
        if expected_len != envelope.len() { return Err(EncryptionError::InvalidFormat); }
        if out.len() < plain_len { return Err(EncryptionError::BufferTooSmall); }

        let tag_offset = envelope.len() - ENCRYPTED_TAG_SIZE_V3;
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&envelope[10..22]);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&envelope[tag_offset..]);

        out[..plain_len].copy_from_slice(&envelope[22..tag_offset]);
        let aad = &envelope[..22];
        let res = ChaCha20Poly1305::open(key, &nonce, aad, &mut out[..plain_len], &tag);
        zeroize_slice(&mut tag);
        zeroize_slice(&mut nonce);
        if res.is_err() {
            zeroize_slice(&mut out[..plain_len]);
            return Err(EncryptionError::IntegrityViolation);
        }
        return Ok(plain_len);
    }

    if envelope.len() < ENCRYPTED_OVERHEAD { return Err(EncryptionError::InvalidFormat); }
    if version != ENCRYPTED_VERSION_V1 && version != ENCRYPTED_VERSION_V2 {
        return Err(EncryptionError::InvalidFormat);
    }
    let plain_len = u32::from_le_bytes([envelope[6], envelope[7], envelope[8], envelope[9]]) as usize;
    let expected_len = ENCRYPTED_OVERHEAD.checked_add(plain_len).ok_or(EncryptionError::InvalidFormat)?;
    if expected_len != envelope.len() { return Err(EncryptionError::InvalidFormat); }
    if out.len() < plain_len { return Err(EncryptionError::BufferTooSmall); }

    let tag_offset = envelope.len() - ENCRYPTED_TAG_SIZE;
    let mut expected_tag = [0u8; 32];
    expected_tag.copy_from_slice(&envelope[tag_offset..]);

    let actual_tag = if version == ENCRYPTED_VERSION_V2 {
        let mut k_mac = HmacSha256::compute(key, DOMAIN_MAC_V2);
        let tag = HmacSha256::compute(&k_mac, &envelope[..tag_offset]);
        zeroize_slice(&mut k_mac);
        tag
    } else {
        HmacSha256::compute(key, &envelope[..tag_offset])
    };

    let auth_ok = constant_time_eq_32(&actual_tag, &expected_tag);
    zeroize_slice(&mut expected_tag);
    if !auth_ok {
        return Err(EncryptionError::IntegrityViolation);
    }

    let nonce = &envelope[10..26];
    let ciphertext = &envelope[26..tag_offset];

    if version == ENCRYPTED_VERSION_V2 {
        let mut k_enc = HmacSha256::compute(key, DOMAIN_ENC_V2);
        let mut stream = [0u8; 32];
        for i in 0..plain_len {
            if i % 32 == 0 {
                let block = (i / 32) as u32;
                let mut input = [0u8; 20];
                input[..16].copy_from_slice(nonce);
                input[16..20].copy_from_slice(&block.to_be_bytes());
                stream = HmacSha256::compute(&k_enc, &input);
                zeroize_slice(&mut input);
            }
            out[i] = ciphertext[i] ^ stream[i % 32];
        }
        zeroize_slice(&mut k_enc);
        zeroize_slice(&mut stream);
    } else {
        let mut stream = [0u8; 32];
        for i in 0..plain_len {
            if i % 32 == 0 {
                let block = (i / 32) as u32;
                let mut input = [0u8; 20];
                input[..16].copy_from_slice(nonce);
                input[16..20].copy_from_slice(&block.to_be_bytes());
                stream = HmacSha256::compute(key, &input);
                zeroize_slice(&mut input);
            }
            out[i] = ciphertext[i] ^ stream[i % 32];
        }
        zeroize_slice(&mut stream);
    }

    Ok(plain_len)
}

/// Authenticates and decrypts an ENC1 envelope.
pub fn open(envelope: &[u8], out: &mut [u8]) -> Result<usize, EncryptionError> {
    // Same refusal as on the write path. Checking the tag under a key derived
    // from an absent root would compare the caller's tag against one anyone can
    // compute, so the "authenticated" verdict would mean nothing.
    let mut key = storage_key()?;
    let res = open_with_key(envelope, &key, out);
    zeroize_slice(&mut key);
    res
}

#[cfg(test)]
pub fn seal_v1_for_test(plaintext: &[u8], key: &[u8; 32], out: &mut Vec<u8>) {
    let mut nonce = [0u8; 16];
    try_get_random_bytes(&mut nonce).unwrap();

    let mut ciphertext = Vec::with_capacity(plaintext.len());
    let mut stream = [0u8; 32];
    for (i, &byte) in plaintext.iter().enumerate() {
        if i % 32 == 0 {
            let block = (i / 32) as u32;
            let mut input = [0u8; 20];
            input[..16].copy_from_slice(&nonce);
            input[16..20].copy_from_slice(&block.to_be_bytes());
            stream = HmacSha256::compute(key, &input);
            zeroize_slice(&mut input);
        }
        ciphertext.push(byte ^ stream[i % 32]);
    }

    out.clear();
    out.reserve(ENCRYPTED_OVERHEAD + ciphertext.len());
    out.extend_from_slice(&ENCRYPTED_MAGIC);
    out.push(ENCRYPTED_VERSION_V1);
    out.push(0);
    out.extend_from_slice(&(plaintext.len() as u32).to_le_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    let tag = HmacSha256::compute(key, out);
    zeroize_slice(&mut stream);
    out.extend_from_slice(&tag);
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let mut envelope = Vec::new();
        envelope.extend_from_slice(&ENCRYPTED_MAGIC);
        envelope.push(ENCRYPTED_VERSION_V2);
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

    const TEST_KEY: [u8; 32] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
        0x55, 0xaa, 0x55, 0xaa, 0x33, 0xcc, 0x33, 0xcc,
        0x0f, 0xf0, 0x0f, 0xf0, 0x7a, 0x8b, 0x9c, 0xad,
    ];

    const WRONG_KEY: [u8; 32] = [
        0x99, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22,
        0x11, 0x00, 0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa,
        0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
        0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78,
    ];

    #[test]
    fn test_v2_seal_and_open_roundtrip() {
        let plaintext = b"InterOS Sovereign Encrypted Payload at rest V2 Envelope";
        let mut envelope = Vec::new();
        assert_eq!(seal_with_key(plaintext, &TEST_KEY, &mut envelope), Ok(()));
        assert_eq!(envelope.len(), ENCRYPTED_OVERHEAD + plaintext.len());
        assert_eq!(envelope[4], ENCRYPTED_VERSION_V2);

        let mut decrypted = alloc::vec![0u8; plaintext.len()];
        let n = open_with_key(&envelope, &TEST_KEY, &mut decrypted).expect("open failed");
        assert_eq!(n, plaintext.len());
        assert_eq!(&decrypted[..n], plaintext);
    }

    #[test]
    fn test_v1_backward_compatibility_migration() {
        let plaintext = b"Legacy Version 1 Envelope Created By Old Kernel Version";
        let mut envelope = Vec::new();
        seal_v1_for_test(plaintext, &TEST_KEY, &mut envelope);
        assert_eq!(envelope[4], ENCRYPTED_VERSION_V1);

        let mut decrypted = alloc::vec![0u8; plaintext.len()];
        let n = open_with_key(&envelope, &TEST_KEY, &mut decrypted).expect("V1 migration open failed");
        assert_eq!(n, plaintext.len());
        assert_eq!(&decrypted[..n], plaintext);
    }

    #[test]
    fn test_wrong_key_fails_closed() {
        let plaintext = b"Payload that must not decrypt with an incorrect key";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        let mut decrypted = alloc::vec![0u8; plaintext.len()];
        assert_eq!(
            open_with_key(&envelope, &WRONG_KEY, &mut decrypted),
            Err(EncryptionError::IntegrityViolation)
        );
    }

    #[test]
    fn test_tampered_header_fails_closed() {
        let plaintext = b"Sensitive payload needing tamper-proof security";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        // 1. Corrupt magic byte
        let mut tampered = envelope.clone();
        tampered[0] ^= 0xFF;
        let mut out = alloc::vec![0u8; plaintext.len()];
        assert_eq!(open_with_key(&tampered, &TEST_KEY, &mut out), Err(EncryptionError::InvalidFormat));

        // 2. Corrupt version byte to unsupported version 3
        let mut tampered = envelope.clone();
        tampered[4] = 3;
        assert_eq!(open_with_key(&tampered, &TEST_KEY, &mut out), Err(EncryptionError::InvalidFormat));

        // 3. Corrupt encoded plaintext length
        let mut tampered = envelope.clone();
        tampered[6] ^= 0x01;
        assert_eq!(open_with_key(&tampered, &TEST_KEY, &mut out), Err(EncryptionError::InvalidFormat));
    }

    #[test]
    fn test_tampered_nonce_fails_closed() {
        let plaintext = b"Sensitive payload with tamperable nonce";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        let mut out = alloc::vec![0u8; plaintext.len()];
        for nonce_offset in 10..26 {
            let mut tampered = envelope.clone();
            tampered[nonce_offset] ^= 0x80;
            assert_eq!(
                open_with_key(&tampered, &TEST_KEY, &mut out),
                Err(EncryptionError::IntegrityViolation),
                "Tampering nonce byte at offset {} must fail closed with IntegrityViolation",
                nonce_offset
            );
        }
    }

    #[test]
    fn test_tampered_ciphertext_fails_closed() {
        let plaintext = b"Secret cryptographic token and user credentials";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        let tag_offset = envelope.len() - ENCRYPTED_TAG_SIZE;
        let mut out = alloc::vec![0u8; plaintext.len()];
        for ct_offset in 26..tag_offset {
            let mut tampered = envelope.clone();
            tampered[ct_offset] ^= 0x01;
            assert_eq!(
                open_with_key(&tampered, &TEST_KEY, &mut out),
                Err(EncryptionError::IntegrityViolation),
                "Tampering ciphertext byte at offset {} must fail closed with IntegrityViolation",
                ct_offset
            );
        }
    }

    #[test]
    fn test_tampered_auth_tag_fails_closed() {
        let plaintext = b"Financial transaction record and auth token";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        let tag_offset = envelope.len() - ENCRYPTED_TAG_SIZE;
        let mut out = alloc::vec![0u8; plaintext.len()];
        for tag_byte in tag_offset..envelope.len() {
            let mut tampered = envelope.clone();
            tampered[tag_byte] ^= 0x40;
            assert_eq!(
                open_with_key(&tampered, &TEST_KEY, &mut out),
                Err(EncryptionError::IntegrityViolation),
                "Tampering HMAC tag byte at offset {} must fail closed with IntegrityViolation",
                tag_byte
            );
        }
    }

    #[test]
    fn test_truncated_envelope_fails_closed() {
        let plaintext = b"Truncation vulnerability test payload";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        let mut out = alloc::vec![0u8; plaintext.len()];
        assert_eq!(
            open_with_key(&envelope[..ENCRYPTED_OVERHEAD - 1], &TEST_KEY, &mut out),
            Err(EncryptionError::InvalidFormat)
        );
        assert_eq!(
            open_with_key(&envelope[..envelope.len() - 1], &TEST_KEY, &mut out),
            Err(EncryptionError::InvalidFormat)
        );
    }

    #[test]
    fn test_buffer_too_small_fails_closed() {
        let plaintext = b"Payload destined for undersized destination buffer";
        let mut envelope = Vec::new();
        seal_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        let mut out = alloc::vec![0u8; plaintext.len() - 1];
        assert_eq!(
            open_with_key(&envelope, &TEST_KEY, &mut out),
            Err(EncryptionError::BufferTooSmall)
        );
    }

    #[test]
    fn test_v3_chacha20poly1305_roundtrip_and_tamper() {
        let plaintext = b"RFC 8439 ChaCha20-Poly1305 authenticated storage envelope";
        let mut envelope = Vec::new();
        seal_v3_with_key(plaintext, &TEST_KEY, &mut envelope).unwrap();

        assert_eq!(envelope[..4], ENCRYPTED_MAGIC);
        assert_eq!(envelope[4], ENCRYPTED_VERSION_V3);

        let mut out = alloc::vec![0u8; plaintext.len()];
        let len = open_with_key(&envelope, &TEST_KEY, &mut out).unwrap();
        assert_eq!(len, plaintext.len());
        assert_eq!(&out[..len], plaintext);

        // Tamper with ciphertext byte
        let mut tampered = envelope.clone();
        tampered[22] ^= 0x01;
        assert_eq!(
            open_with_key(&tampered, &TEST_KEY, &mut out),
            Err(EncryptionError::IntegrityViolation)
        );

        // Tamper with Poly1305 tag
        let mut tampered_tag = envelope.clone();
        let last = tampered_tag.len() - 1;
        tampered_tag[last] ^= 0x80;
        assert_eq!(
            open_with_key(&tampered_tag, &TEST_KEY, &mut out),
            Err(EncryptionError::IntegrityViolation)
        );
    }
}
