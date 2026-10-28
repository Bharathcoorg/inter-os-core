//! HMAC-SHA256 Message Authentication Code (RFC 2104 / RFC 4231)
//!
//! Provides cryptographic authentication and key derivation for TLS 1.3
//! Finished verify_data and HKDF in pure `no_std` Rust with zero allocations.

use super::sha256::{Sha256, SHA256_BLOCK_SIZE, SHA256_DIGEST_LENGTH};

/// Streaming HMAC-SHA256 engine.
#[derive(Clone, Copy)]
pub struct HmacSha256 {
    inner: Sha256,
    opad: [u8; SHA256_BLOCK_SIZE],
}

impl HmacSha256 {
    /// Initializes a new HMAC-SHA256 session with the given key.
    pub fn new(key: &[u8]) -> Self {
        let mut k = [0u8; SHA256_BLOCK_SIZE];

        if key.len() > SHA256_BLOCK_SIZE {
            let key_digest = Sha256::digest(key);
            k[..SHA256_DIGEST_LENGTH].copy_from_slice(&key_digest);
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        let mut ipad = [0u8; SHA256_BLOCK_SIZE];
        let mut opad = [0u8; SHA256_BLOCK_SIZE];

        for i in 0..SHA256_BLOCK_SIZE {
            ipad[i] = k[i] ^ 0x36;
            opad[i] = k[i] ^ 0x5c;
        }

        let mut inner = Sha256::new();
        inner.update(&ipad);

        Self { inner, opad }
    }

    /// Feeds data into the HMAC calculation.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finalizes and returns the 32-byte HMAC tag.
    pub fn finalize(self) -> [u8; SHA256_DIGEST_LENGTH] {
        let inner_digest = self.inner.finalize();
        let mut outer = Sha256::new();
        outer.update(&self.opad);
        outer.update(&inner_digest);
        outer.finalize()
    }

    /// Computes HMAC-SHA256 in one shot.
    pub fn compute(key: &[u8], message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut hmac = Self::new(key);
        hmac.update(message);
        hmac.finalize()
    }

    /// Constant-time comparison of two HMAC tags to prevent timing attacks.
    pub fn verify(key: &[u8], message: &[u8], expected_tag: &[u8; SHA256_DIGEST_LENGTH]) -> bool {
        let actual_tag = Self::compute(key, message);
        let mut diff = 0u8;
        for i in 0..SHA256_DIGEST_LENGTH {
            diff |= actual_tag[i] ^ expected_tag[i];
        }
        diff == 0
    }
}
