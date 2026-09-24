//! Cryptographic Subsystem for Inter OS
//!
//! Provides bare-metal cryptographic primitives for Merkle tree rootfs verification,
//! `.rapp` package attestation, and sovereign identity state hashing.

pub mod hmac;
pub mod keystore;
pub mod rng;
pub mod sha256;

pub use hmac::HmacSha256;
pub use keystore::{KeySlot, KeystoreError, SecureKeystore, SECURE_KEYSTORE};
pub use rng::{get_random_bytes, HardwareRng, TRNG};
pub use sha256::{Sha256, SHA256_BLOCK_SIZE, SHA256_DIGEST_LENGTH};

/// Constant-time 32-byte array equality comparison preventing timing attacks.
#[inline(never)]
pub fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

