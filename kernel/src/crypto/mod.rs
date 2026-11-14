// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Cryptographic Subsystem for Inter OS
//!
//! Provides bare-metal cryptographic primitives for Merkle tree rootfs verification,
//! `.rapp` package attestation, and sovereign identity state hashing.

pub mod ed25519;
pub mod hmac;
pub mod keystore;
pub mod rng;
pub mod sha256;
pub mod tls_trust;

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
    core::hint::black_box(diff) == 0
}

/// Constant-time 4-byte array equality comparison (e.g. for SIM PINs).
#[inline(never)]
pub fn constant_time_eq_4(a: &[u8; 4], b: &[u8; 4]) -> bool {
    let diff = (a[0] ^ b[0]) | (a[1] ^ b[1]) | (a[2] ^ b[2]) | (a[3] ^ b[3]);
    core::hint::black_box(diff) == 0
}

/// Constant-time slice equality comparison preventing timing side-channels.
#[inline(never)]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    core::hint::black_box(diff) == 0
}

/// Securely wipes a memory slice with zeros using volatile writes and memory barriers,
/// guaranteeing that LLVM cannot eliminate the operation via Dead Store Elimination (DSE).
#[inline(never)]
pub fn zeroize_slice(buffer: &mut [u8]) {
    for byte in buffer.iter_mut() {
        unsafe {
            core::ptr::write_volatile(byte, 0);
        }
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    core::hint::black_box(buffer.as_mut_ptr());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_32() {
        let a = [0x5Au8; 32];
        let b = [0x5Au8; 32];
        assert!(constant_time_eq_32(&a, &b));

        for i in 0..32 {
            let mut c = a;
            c[i] ^= 0x01;
            assert!(!constant_time_eq_32(&a, &c));
        }
    }

    #[test]
    fn test_constant_time_eq_4() {
        let a = [1, 2, 3, 4];
        let b = [1, 2, 3, 4];
        assert!(constant_time_eq_4(&a, &b));

        let c = [1, 2, 3, 5];
        assert!(!constant_time_eq_4(&a, &c));
    }

    #[test]
    fn test_constant_time_eq() {
        let a = [10, 20, 30, 40, 50];
        let b = [10, 20, 30, 40, 50];
        assert!(constant_time_eq(&a, &b));

        let c = [10, 20, 30, 40, 51];
        assert!(!constant_time_eq(&a, &c));

        let d = [10, 20, 30];
        assert!(!constant_time_eq(&a, &d));
    }

    #[test]
    fn test_zeroize_slice() {
        let mut data = [0xAAu8; 64];
        zeroize_slice(&mut data);
        assert!(data.iter().all(|&b| b == 0));
    }
}

