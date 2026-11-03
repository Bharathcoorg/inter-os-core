// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! HMAC-SHA256 Message Authentication Code (RFC 2104 / RFC 4231)
//!
//! Provides cryptographic signatures and attestation for sovereign device
//! identities and state delta commitments in pure `no_std` Rust.

use alloc::vec::Vec;
use super::sha256::{Sha256, SHA256_BLOCK_SIZE, SHA256_DIGEST_LENGTH};

/// HMAC-SHA256 engine.
pub struct HmacSha256;

impl HmacSha256 {
    /// Alias for mac(). Computes the HMAC-SHA256 authentication tag for a given key and message.
    #[inline]
    pub fn compute(key: &[u8], message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        Self::mac(key, message)
    }

    /// Computes the HMAC-SHA256 authentication tag for a given key and message.
    pub fn mac(key: &[u8], message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut k = [0u8; SHA256_BLOCK_SIZE];

        if key.len() > SHA256_BLOCK_SIZE {
            let key_digest = Sha256::digest(key);
            k[..SHA256_DIGEST_LENGTH].copy_from_slice(&key_digest);
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        let mut k_ipad = [0u8; SHA256_BLOCK_SIZE];
        let mut k_opad = [0u8; SHA256_BLOCK_SIZE];

        for i in 0..SHA256_BLOCK_SIZE {
            k_ipad[i] = k[i] ^ 0x36;
            k_opad[i] = k[i] ^ 0x5C;
        }

        // Inner hash: H(K_ipad || message)
        let mut inner_input = Vec::with_capacity(SHA256_BLOCK_SIZE + message.len());
        inner_input.extend_from_slice(&k_ipad);
        inner_input.extend_from_slice(message);
        let inner_digest = Sha256::digest(&inner_input);

        // Outer hash: H(K_opad || inner_digest)
        let mut outer_input = Vec::with_capacity(SHA256_BLOCK_SIZE + SHA256_DIGEST_LENGTH);
        outer_input.extend_from_slice(&k_opad);
        outer_input.extend_from_slice(&inner_digest);
        Sha256::digest(&outer_input)
    }

    /// Constant-time comparison of two HMAC authentication tags to prevent timing attacks.
    pub fn verify(key: &[u8], message: &[u8], expected_tag: &[u8; SHA256_DIGEST_LENGTH]) -> bool {
        let actual_tag = Self::mac(key, message);
        let mut diff = 0u8;
        for i in 0..SHA256_DIGEST_LENGTH {
            diff |= actual_tag[i] ^ expected_tag[i];
        }
        diff == 0
    }
}
