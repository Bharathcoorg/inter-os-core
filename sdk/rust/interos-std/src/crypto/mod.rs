// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Cryptographic Primitives for InterOS Userspace and TLS 1.3
//!
//! Provides deterministic, constant-time, `#![no_std]` implementations of:
//! - SHA-256 (FIPS PUB 180-4)
//! - SHA-384 (FIPS PUB 180-4)
//! - HMAC-SHA256 (RFC 2104 / RFC 4231)
//! - HKDF-SHA256 (RFC 5869) and TLS 1.3 HKDF-Expand-Label (RFC 8446 Section 7.1)
//! - X25519 (RFC 7748) Montgomery curve scalar multiplication
//! - ChaCha20-Poly1305 AEAD (RFC 8439)
//!
//! The signature checkers P-256 and P-384 ECDSA (FIPS PUB 186-5) and RSA
//! (PKCS#1 v2.2) are verification-only and `no_std`, but not constant-time: they
//! only ever handle public keys and other peers' signatures, so no secret is
//! available to time.
//! - ChaCha20-Poly1305 AEAD (RFC 8439)
//!
//! All implementations are pure Rust with zero third-party dependencies and zero heap allocations.

pub mod chacha20_poly1305;
pub mod hkdf;
pub mod hmac;
pub mod p256;
pub mod p384;
pub mod rsa;
pub mod sha256;
pub mod sha384;
pub mod x25519;
pub use chacha20_poly1305::{ChaCha20Poly1305, Poly1305};
pub use hkdf::{derive_secret, hkdf_expand, hkdf_expand_label, hkdf_extract};
pub use hmac::HmacSha256;
pub use sha256::Sha256;
pub use sha384::Sha384;
pub use x25519::{x25519, x25519_base, BASE_POINT};
