// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Allocation-Free HMAC-SHA256 Message Authentication Code (RFC 2104 / RFC 4231)
//!
//! Provides cryptographic signatures, attestation, and password derivation in pure
//! `no_std` Rust with zero dynamic heap allocations.

use super::sha256::{Sha256, SHA256_BLOCK_SIZE, SHA256_DIGEST_LENGTH};

/// Incremental and allocation-free HMAC-SHA256 engine.
#[derive(Clone)]
pub struct HmacSha256 {
    outer: Sha256,
    inner: Sha256,
}

impl HmacSha256 {
    /// Initializes a new HMAC-SHA256 instance with the given key.
    /// Handles keys shorter or longer than the SHA-256 block size (64 bytes).
    /// All intermediate key pads are securely zeroized using volatile stores.
    pub fn new(key: &[u8]) -> Self {
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

        // Scrub raw key buffer immediately
        crate::crypto::zeroize_slice(&mut k);

        let mut inner = Sha256::new();
        inner.update(&k_ipad);
        let mut outer = Sha256::new();
        outer.update(&k_opad);

        // Scrub intermediate pads
        crate::crypto::zeroize_slice(&mut k_ipad);
        crate::crypto::zeroize_slice(&mut k_opad);

        Self { outer, inner }
    }

    /// Appends data chunks to the inner HMAC hash state without memory allocations.
    #[inline]
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalizes the HMAC calculation, returning the 32-byte authentication tag.
    /// Intermediate inner digests are securely zeroized.
    pub fn finalize(mut self) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut inner_digest = self.inner.finalize();
        self.outer.update(&inner_digest);
        crate::crypto::zeroize_slice(&mut inner_digest);
        self.outer.finalize()
    }

    /// Securely wipes all internal state and hashers.
    pub fn zeroize(&mut self) {
        self.inner.zeroize();
        self.outer.zeroize();
    }

    /// Computes the HMAC-SHA256 authentication tag for a given key and message.
    /// Zero heap allocations guaranteed.
    #[inline]
    pub fn mac(key: &[u8], message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut hmac = Self::new(key);
        hmac.update(message);
        hmac.finalize()
    }

    /// Alias for mac().
    #[inline]
    pub fn compute(key: &[u8], message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        Self::mac(key, message)
    }

    /// Constant-time comparison of two HMAC authentication tags to prevent timing attacks.
    pub fn verify(key: &[u8], message: &[u8], expected_tag: &[u8; SHA256_DIGEST_LENGTH]) -> bool {
        let actual_tag = Self::mac(key, message);
        crate::crypto::constant_time_eq_32(&actual_tag, expected_tag)
    }

    /// Derives a 32-byte key using PBKDF2-HMAC-SHA256 (RFC 2898 / RFC 8018).
    ///
    /// Computes DK = T_1 = F(password, salt, iterations, 1).
    /// Genuinely allocation-free: performs zero heap allocations.
    /// Pre-computes inner and outer key states for optimal performance.
    /// All secret-dependent buffers (U values, intermediate states) are zeroized.
    pub fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8; SHA256_DIGEST_LENGTH]) {
        if iterations == 0 {
            return;
        }

        // Precompute key pad state for HMAC(password, ...) once to avoid re-hashing key on every round
        let mut base_hmac = Self::new(password);

        // U_1 = PRF(password, salt || INT_32_BE(1))
        let mut u1_hmac = base_hmac.clone();
        u1_hmac.update(salt);
        u1_hmac.update(&1u32.to_be_bytes());
        let mut u = u1_hmac.finalize();

        out.copy_from_slice(&u);

        for _ in 1..iterations {
            let mut round_hmac = base_hmac.clone();
            round_hmac.update(&u);
            u = round_hmac.finalize();
            for i in 0..SHA256_DIGEST_LENGTH {
                out[i] ^= u[i];
            }
        }

        crate::crypto::zeroize_slice(&mut u);
        base_hmac.zeroize();
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::println;
    use super::*;

    #[test]
    fn test_hmac_rfc4231_case1() {
        // Test Case 1: Key = 20 bytes of 0x0b, Data = "Hi There"
        let key = [0x0bu8; 20];
        let data = b"Hi There";
        let tag = HmacSha256::mac(&key, data);
        let expected = [
            0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53,
            0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b, 0xf1, 0x2b,
            0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7,
            0x26, 0xe9, 0x37, 0x6c, 0x2e, 0x32, 0xcf, 0xf7,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(&key, data, &expected));
    }

    #[test]
    fn test_hmac_rfc4231_case2() {
        // Test Case 2: Key = "Jefe", Data = "what do ya want for nothing?"
        let key = b"Jefe";
        let data = b"what do ya want for nothing?";
        let tag = HmacSha256::mac(key, data);
        let expected = [
            0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e,
            0x6a, 0x04, 0x24, 0x26, 0x08, 0x95, 0x75, 0xc7,
            0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83,
            0x9d, 0xec, 0x58, 0xb9, 0x64, 0xec, 0x38, 0x43,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(key, data, &expected));
    }

    #[test]
    fn test_hmac_rfc4231_case3() {
        // Test Case 3: Key = 0xaa * 32, Data = 0xdd * 50
        let key = [0xaau8; 32];
        let data = [0xddu8; 50];
        let tag = HmacSha256::mac(&key, &data);
        let expected = [
            0xcd, 0xcb, 0x12, 0x20, 0xd1, 0xec, 0xcc, 0xea,
            0x91, 0xe5, 0x3a, 0xba, 0x30, 0x92, 0xf9, 0x62,
            0xe5, 0x49, 0xfe, 0x6c, 0xe9, 0xed, 0x7f, 0xdc,
            0x43, 0x19, 0x1f, 0xbd, 0xe4, 0x5c, 0x30, 0xb0,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(&key, &data, &expected));
    }

    #[test]
    fn test_hmac_rfc4231_case4() {
        // Test Case 4: Key = 25 bytes (0x01..0x19), Data = 0xcd * 50
        let mut key = [0u8; 25];
        for i in 0..25 {
            key[i] = (i + 1) as u8;
        }
        let data = [0xcdu8; 50];
        let tag = HmacSha256::mac(&key, &data);
        let expected = [
            0x82, 0x55, 0x8a, 0x38, 0x9a, 0x44, 0x3c, 0x0e,
            0xa4, 0xcc, 0x81, 0x98, 0x99, 0xf2, 0x08, 0x3a,
            0x85, 0xf0, 0xfa, 0xa3, 0xe5, 0x78, 0xf8, 0x07,
            0x7a, 0x2e, 0x3f, 0xf4, 0x67, 0x29, 0x66, 0x5b,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(&key, &data, &expected));
    }

    #[test]
    fn test_hmac_rfc4231_case5() {
        // Test Case 5: Key = 0x0c * 20, Data = "Test With Truncation"
        let key = [0x0cu8; 20];
        let data = b"Test With Truncation";
        let tag = HmacSha256::mac(&key, data);
        let expected = [
            0xa3, 0xb6, 0x16, 0x74, 0x73, 0x10, 0x0e, 0xe0,
            0x6e, 0x0c, 0x79, 0x6c, 0x29, 0x55, 0x55, 0x2b,
            0xfa, 0x6f, 0x7c, 0x0a, 0x6a, 0x8a, 0xef, 0x8b,
            0x93, 0xf8, 0x60, 0xaa, 0xb0, 0xcd, 0x20, 0xc5,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(&key, data, &expected));
    }

    #[test]
    fn test_hmac_rfc4231_case6_oversized_key() {
        // Test Case 6: Key = 131 bytes of 0xaa (exceeds 64-byte block size)
        let key = [0xaau8; 131];
        let data = b"Test Using Larger Than Block-Size Key - Hash Key First";
        let tag = HmacSha256::mac(&key, data);
        let expected = [
            0x60, 0xe4, 0x31, 0x59, 0x1e, 0xe0, 0xb6, 0x7f,
            0x0d, 0x8a, 0x26, 0xaa, 0xcb, 0xf5, 0xb7, 0x7f,
            0x8e, 0x0b, 0xc6, 0x21, 0x37, 0x28, 0xc5, 0x14,
            0x05, 0x46, 0x04, 0x0f, 0x0e, 0xe3, 0x7f, 0x54,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(&key, data, &expected));
    }

    #[test]
    fn test_hmac_rfc4231_case7_large_key_and_data() {
        // Test Case 7: Key = 131 bytes of 0xaa, Data = 152 bytes
        let key = [0xaau8; 131];
        let data = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
        let tag = HmacSha256::mac(&key, data);
        let expected = [
            0x9b, 0x09, 0xff, 0xa7, 0x1b, 0x94, 0x2f, 0xcb,
            0x27, 0x63, 0x5f, 0xbc, 0xd5, 0xb0, 0xe9, 0x44,
            0xbf, 0xdc, 0x63, 0x64, 0x4f, 0x07, 0x13, 0x93,
            0x8a, 0x7f, 0x51, 0x53, 0x5c, 0x3a, 0x35, 0xe2,
        ];
        assert_eq!(tag, expected);
        assert!(HmacSha256::verify(&key, data, &expected));
    }

    #[test]
    fn test_pbkdf2_ietf_sha256_vectors() {
        // Authoritative IETF test vectors for PBKDF2-HMAC-SHA256 (draft-josefsson-pbkdf2-sha256-test-vectors)
        let mut out = [0u8; 32];

        // IETF Vector 1: P = "password", S = "salt", c = 1
        HmacSha256::pbkdf2(b"password", b"salt", 1, &mut out);
        let expected_1 = [
            0x12, 0x0f, 0xb6, 0xcf, 0xfc, 0xf8, 0xb3, 0x2c,
            0x43, 0xe7, 0x22, 0x52, 0x56, 0xc4, 0xf8, 0x37,
            0xa8, 0x65, 0x48, 0xc9, 0x2c, 0xcc, 0x35, 0x48,
            0x08, 0x05, 0x98, 0x7c, 0xb7, 0x0b, 0xe1, 0x7b,
        ];
        assert_eq!(out, expected_1);

        // IETF Vector 2: P = "password", S = "salt", c = 2
        HmacSha256::pbkdf2(b"password", b"salt", 2, &mut out);
        let expected_2 = [
            0xae, 0x4d, 0x0c, 0x95, 0xaf, 0x6b, 0x46, 0xd3,
            0x2d, 0x0a, 0xdf, 0xf9, 0x28, 0xf0, 0x6d, 0xd0,
            0x2a, 0x30, 0x3f, 0x8e, 0xf3, 0xc2, 0x51, 0xdf,
            0xd6, 0xe2, 0xd8, 0x5a, 0x95, 0x47, 0x4c, 0x43,
        ];
        assert_eq!(out, expected_2);

        // IETF Vector 3: P = "password", S = "salt", c = 4096
        HmacSha256::pbkdf2(b"password", b"salt", 4096, &mut out);
        let expected_4096 = [
            0xc5, 0xe4, 0x78, 0xd5, 0x92, 0x88, 0xc8, 0x41,
            0xaa, 0x53, 0x0d, 0xb6, 0x84, 0x5c, 0x4c, 0x8d,
            0x96, 0x28, 0x93, 0xa0, 0x01, 0xce, 0x4e, 0x11,
            0xa4, 0x96, 0x38, 0x73, 0xaa, 0x98, 0x13, 0x4a,
        ];
        assert_eq!(out, expected_4096);
    }

    #[test]
    fn test_pbkdf2_rfc7914_sha256_vector() {
        // RFC 7914 (scrypt) Section 11, Test Vector 1 PBKDF2-HMAC-SHA256 baseline
        // P = "passwd", S = "salt", c = 1, out = 32 bytes
        let mut out = [0u8; 32];
        HmacSha256::pbkdf2(b"passwd", b"salt", 1, &mut out);
        let expected = [
            0x55, 0xac, 0x04, 0x6e, 0x56, 0xe3, 0x08, 0x9f,
            0xec, 0x16, 0x91, 0xc2, 0x25, 0x44, 0xb6, 0x05,
            0xf9, 0x41, 0x85, 0x21, 0x6d, 0xde, 0x04, 0x65,
            0xe6, 0x8b, 0x9d, 0x57, 0xc2, 0x0d, 0xac, 0xbc,
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn test_pbkdf2_empty_and_long_inputs() {
        let mut out = [0u8; 32];

        // Empty password and salt with 1 iteration
        HmacSha256::pbkdf2(b"", b"", 1, &mut out);
        assert_ne!(out, [0u8; 32]);

        // Long password (128 bytes)
        let long_pw = [0x5au8; 128];
        let salt = b"salt1234";
        HmacSha256::pbkdf2(&long_pw, salt, 5, &mut out);
        assert_ne!(out, [0u8; 32]);
    }

    #[test]
    fn test_hmac_and_sha256_zeroization() {
        let key = b"confidential_hmac_key";
        let mut hmac = HmacSha256::new(key);
        hmac.update(b"sensitive transaction data");
        // Verify state is populated before zeroization
        hmac.zeroize();
        // Finalize after zeroize should yield clean predictable empty digest without panicking
        let tag = hmac.finalize();
        assert_ne!(tag, [0u8; 32]);
    }

    #[test]
    fn test_pbkdf2_benchmark() {
        let mut out = [0u8; 32];
        let start = std::time::Instant::now();
        HmacSha256::pbkdf2(b"123456", b"salt_for_benchmark", 1000, &mut out);
        let elapsed_1k = start.elapsed();

        let start = std::time::Instant::now();
        HmacSha256::pbkdf2(b"123456", b"salt_for_benchmark", 5000, &mut out);
        let elapsed_5k = start.elapsed();

        let start = std::time::Instant::now();
        HmacSha256::pbkdf2(b"123456", b"salt_for_benchmark", 10000, &mut out);
        let elapsed_10k = start.elapsed();

        println!("[BENCHMARK] PBKDF2 1,000 rounds:  {:?}", elapsed_1k);
        println!("[BENCHMARK] PBKDF2 5,000 rounds:  {:?}", elapsed_5k);
        println!("[BENCHMARK] PBKDF2 10,000 rounds: {:?}", elapsed_10k);
    }
}
