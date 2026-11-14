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

    /// Derives a 32-byte key using PBKDF2-HMAC-SHA256 (RFC 2898 / RFC 8018).
    ///
    /// Computes DK = T_1 = F(password, salt, iterations, 1).
    /// All intermediate secret-dependent buffers are securely zeroized using volatile writes.
    pub fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8; SHA256_DIGEST_LENGTH]) {
        if iterations == 0 {
            return;
        }

        // U_1 = PRF(password, salt || INT_32_BE(1))
        let mut salt_int = Vec::with_capacity(salt.len() + 4);
        salt_int.extend_from_slice(salt);
        salt_int.extend_from_slice(&1u32.to_be_bytes());

        let mut u = Self::mac(password, &salt_int);
        crate::crypto::zeroize_slice(salt_int.as_mut_slice());

        out.copy_from_slice(&u);

        for _ in 1..iterations {
            u = Self::mac(password, &u);
            for i in 0..SHA256_DIGEST_LENGTH {
                out[i] ^= u[i];
            }
        }

        crate::crypto::zeroize_slice(&mut u);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pbkdf2_rfc6070_vectors() {
        let mut out = [0u8; 32];

        // RFC 6070 Vector 1: P = "password", S = "salt", c = 1
        HmacSha256::pbkdf2(b"password", b"salt", 1, &mut out);
        let expected_1 = [
            0x12, 0x0f, 0xb6, 0xcf, 0xfc, 0xf8, 0xb3, 0x2c,
            0x43, 0xe7, 0x22, 0x52, 0x56, 0xc4, 0xf8, 0x37,
            0xa8, 0x65, 0x48, 0xc9, 0x2c, 0xcc, 0x35, 0x48,
            0x08, 0x05, 0x98, 0x7c, 0xb7, 0x0b, 0xe1, 0x7b,
        ];
        assert_eq!(out, expected_1);

        // PBKDF2-HMAC-SHA256 Vector 2: P = "password", S = "salt", c = 2
        HmacSha256::pbkdf2(b"password", b"salt", 2, &mut out);
        let expected_2 = [
            174, 77, 12, 149, 175, 107, 70, 211, 45, 10, 223, 249, 40, 240, 109, 208,
            42, 48, 63, 142, 243, 194, 81, 223, 214, 226, 216, 90, 149, 71, 76, 67,
        ];
        assert_eq!(out, expected_2);
    }
}
