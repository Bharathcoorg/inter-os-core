//! Sovereign Decentralized Identifier (DID) Implementation
//!
//! Conforms to the W3C DID Core specification with the `did:inter` method scheme:
//! `did:inter:<32-hex-character-device-fingerprint>`
//!
//! Provides cryptographic device identification without central authority or cloud registry.

use alloc::string::String;
use alloc::format;
use core::fmt;

/// Maximum length of a DID string (`did:inter:` + 32 hex chars = 42 chars).
pub const DID_MAX_LENGTH: usize = 42;

/// Strongly-typed sovereign Decentralized Identifier.
#[derive(Clone, PartialEq, Eq)]
pub struct SovereignDid {
    raw: String,
    fingerprint: [u8; 16],
}

impl SovereignDid {
    /// Derives a Sovereign DID from a 32-byte public identity hash.
    pub fn from_pubkey_hash(hash: &[u8; 32]) -> Self {
        let mut fingerprint = [0u8; 16];
        fingerprint.copy_from_slice(&hash[..16]);

        let mut hex_str = String::with_capacity(32);
        for byte in &fingerprint {
            use core::fmt::Write;
            let _ = write!(hex_str, "{:02x}", byte);
        }

        let raw = format!("did:inter:{}", hex_str);
        Self { raw, fingerprint }
    }

    /// Creates a Sovereign DID from an existing raw string (verifying prefix and length).
    pub fn parse(s: &str) -> Result<Self, &'static str> {
        if !s.starts_with("did:inter:") {
            return Err("Invalid DID method scheme: must start with 'did:inter:'");
        }
        let hex_part = &s[10..];
        if hex_part.len() != 32 {
            return Err("Invalid DID fingerprint length: expected 32 hex characters");
        }

        let mut fingerprint = [0u8; 16];
        for i in 0..16 {
            let byte_str = &hex_part[i * 2..i * 2 + 2];
            fingerprint[i] = u8::from_str_radix(byte_str, 16)
                .map_err(|_| "Invalid hex character in DID fingerprint")?;
        }

        Ok(Self {
            raw: String::from(s),
            fingerprint,
        })
    }

    /// Returns the raw DID string slice.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Returns the 16-byte raw fingerprint.
    pub fn fingerprint(&self) -> &[u8; 16] {
        &self.fingerprint
    }
}

impl fmt::Display for SovereignDid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

impl fmt::Debug for SovereignDid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SovereignDid({})", self.raw)
    }
}
