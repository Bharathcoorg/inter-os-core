//! `.rapp` — Rust App Package Format & Verifier
//!
//! Defines the binary structure of `.rapp` packages for distributing native
//! InterOS applications. Provides:
//! - Binary header with magic, version, and architecture target
//! - SHA-256 payload integrity digest
//! - Ed25519 signature slot for publisher attestation (NOT_IMPLEMENTED on QEMU)
//! - Capability manifest declaring required permissions
//! - ELF binary payload for direct loading into per-process address spaces
//!
//! ## Binary Layout
//!
//! ```text
//! Offset  Size    Field
//! 0x00    4       Magic: "RAPP"
//! 0x04    2       Format version (LE u16)
//! 0x06    2       Target architecture (0=x86_64, 1=aarch64, 2=riscv64)
//! 0x08    32      Package ID (UTF-8, null-padded)
//! 0x28    16      Publisher ID (UTF-8, null-padded)
//! 0x38    2       Version major (LE u16)
//! 0x3A    2       Version minor (LE u16)
//! 0x3C    2       Version patch (LE u16)
//! 0x3E    2       Capability count (LE u16)
//! 0x40    32      SHA-256 digest of ELF payload
//! 0x60    64      Ed25519 signature (zeros if unsigned)
//! 0xA0    4       ELF payload length (LE u32)
//! 0xA4    N*2     Capability IDs (N entries, LE u16 each)
//! 0xA4+N*2  ...   ELF payload bytes
//! ```

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};

/// Magic header bytes identifying a `.rapp` package.
pub const RAPP_MAGIC: &[u8; 4] = b"RAPP";

/// Current `.rapp` format version.
pub const RAPP_FORMAT_VERSION: u16 = 1;

/// Minimum header size before capability array and payload.
pub const RAPP_HEADER_SIZE: usize = 0xA4;

/// Target CPU architecture for the ELF payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RappArch {
    X86_64 = 0,
    Aarch64 = 1,
    Riscv64 = 2,
}

impl RappArch {
    pub fn matches_current(self) -> bool {
        #[cfg(target_arch = "x86_64")]
        { return self == Self::X86_64; }
        #[cfg(target_arch = "aarch64")]
        { return self == Self::Aarch64; }
        #[cfg(target_arch = "riscv64")]
        { return self == Self::Riscv64; }
        #[allow(unreachable_code)]
        false
    }

    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0 => Some(Self::X86_64),
            1 => Some(Self::Aarch64),
            2 => Some(Self::Riscv64),
            _ => None,
        }
    }
}

/// Errors during `.rapp` package parsing or verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RappError {
    /// Buffer too short for header.
    BufferTooShort,
    /// Magic bytes do not match "RAPP".
    InvalidMagic,
    /// Unsupported format version.
    UnsupportedVersion,
    /// Target architecture not recognized.
    UnsupportedArch,
    /// SHA-256 digest of payload does not match header.
    DigestMismatch,
    /// Ed25519 signature verification failed.
    SignatureInvalid,
    /// Payload length in header exceeds remaining buffer.
    PayloadOverflow,
    /// Declared capability is not recognized.
    UnknownCapability,
}

/// Verified `.rapp` package metadata.
#[derive(Debug, Clone)]
pub struct RappManifest {
    pub package_id: String,
    pub publisher_id: String,
    pub version: (u16, u16, u16),
    pub arch: RappArch,
    pub capabilities: Vec<u16>,
    pub payload_digest: [u8; SHA256_DIGEST_LENGTH],
    pub signature: [u8; 64],
    pub payload_offset: usize,
    pub payload_len: usize,
}

/// Parses and verifies a `.rapp` binary blob.
///
/// Returns a `RappManifest` with payload offset and length on success.
/// The caller can then use `payload_offset..payload_offset+payload_len`
/// to extract the ELF binary for loading.
pub fn parse_rapp(data: &[u8]) -> Result<RappManifest, RappError> {
    if data.len() < RAPP_HEADER_SIZE {
        return Err(RappError::BufferTooShort);
    }

    // 1. Magic
    if &data[0..4] != RAPP_MAGIC {
        return Err(RappError::InvalidMagic);
    }

    // 2. Format version
    let version = u16::from_le_bytes([data[4], data[5]]);
    if version != RAPP_FORMAT_VERSION {
        return Err(RappError::UnsupportedVersion);
    }

    // 3. Architecture
    let arch_raw = u16::from_le_bytes([data[6], data[7]]);
    let arch = RappArch::from_u16(arch_raw).ok_or(RappError::UnsupportedArch)?;

    // 4. Package ID (32 bytes, null-padded UTF-8)
    let pkg_bytes = &data[0x08..0x28];
    let pkg_end = pkg_bytes.iter().position(|&b| b == 0).unwrap_or(32);
    let package_id = core::str::from_utf8(&pkg_bytes[..pkg_end])
        .unwrap_or("unknown")
        .to_string();

    // 5. Publisher ID (16 bytes, null-padded UTF-8)
    let pub_bytes = &data[0x28..0x38];
    let pub_end = pub_bytes.iter().position(|&b| b == 0).unwrap_or(16);
    let publisher_id = core::str::from_utf8(&pub_bytes[..pub_end])
        .unwrap_or("unknown")
        .to_string();

    // 6. Version tuple
    let v_major = u16::from_le_bytes([data[0x38], data[0x39]]);
    let v_minor = u16::from_le_bytes([data[0x3A], data[0x3B]]);
    let v_patch = u16::from_le_bytes([data[0x3C], data[0x3D]]);

    // 7. Capability count
    let cap_count = u16::from_le_bytes([data[0x3E], data[0x3F]]) as usize;

    // 8. SHA-256 digest
    let mut payload_digest = [0u8; SHA256_DIGEST_LENGTH];
    payload_digest.copy_from_slice(&data[0x40..0x60]);

    // 9. Ed25519 signature
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&data[0x60..0xA0]);

    // 10. Payload length
    let payload_len = u32::from_le_bytes([data[0xA0], data[0xA1], data[0xA2], data[0xA3]]) as usize;

    // 11. Capabilities array
    let caps_start = RAPP_HEADER_SIZE;
    let caps_end = caps_start + cap_count * 2;
    if data.len() < caps_end {
        return Err(RappError::BufferTooShort);
    }
    let mut capabilities = Vec::with_capacity(cap_count);
    for i in 0..cap_count {
        let offset = caps_start + i * 2;
        capabilities.push(u16::from_le_bytes([data[offset], data[offset + 1]]));
    }

    // 12. Payload bounds check
    let payload_offset = caps_end;
    if data.len() < payload_offset + payload_len {
        return Err(RappError::PayloadOverflow);
    }

    // 13. Verify SHA-256 digest
    let computed_digest = Sha256::digest(&data[payload_offset..payload_offset + payload_len]);
    if computed_digest != payload_digest {
        return Err(RappError::DigestMismatch);
    }

    // 14. Signature verification: NOT_IMPLEMENTED in development QEMU targets.
    // In production, verify Ed25519 signature over header + payload here.
    // For now, accept unsigned packages (signature = all zeros).

    Ok(RappManifest {
        package_id,
        publisher_id,
        version: (v_major, v_minor, v_patch),
        arch,
        capabilities,
        payload_digest,
        signature,
        payload_offset,
        payload_len,
    })
}

/// Builds a `.rapp` binary package from components.
pub fn build_rapp(
    package_id: &str,
    publisher_id: &str,
    version: (u16, u16, u16),
    arch: RappArch,
    capabilities: &[u16],
    elf_payload: &[u8],
) -> Vec<u8> {
    let digest = Sha256::digest(elf_payload);
    let mut buf = Vec::with_capacity(RAPP_HEADER_SIZE + capabilities.len() * 2 + elf_payload.len());

    // Magic
    buf.extend_from_slice(RAPP_MAGIC);
    // Format version
    buf.extend_from_slice(&RAPP_FORMAT_VERSION.to_le_bytes());
    // Architecture
    buf.extend_from_slice(&(arch as u16).to_le_bytes());

    // Package ID (32 bytes, null-padded)
    let mut pkg_buf = [0u8; 32];
    let pkg_bytes = package_id.as_bytes();
    let copy_len = pkg_bytes.len().min(32);
    pkg_buf[..copy_len].copy_from_slice(&pkg_bytes[..copy_len]);
    buf.extend_from_slice(&pkg_buf);

    // Publisher ID (16 bytes, null-padded)
    let mut pub_buf = [0u8; 16];
    let pub_bytes = publisher_id.as_bytes();
    let copy_len = pub_bytes.len().min(16);
    pub_buf[..copy_len].copy_from_slice(&pub_bytes[..copy_len]);
    buf.extend_from_slice(&pub_buf);

    // Version
    buf.extend_from_slice(&version.0.to_le_bytes());
    buf.extend_from_slice(&version.1.to_le_bytes());
    buf.extend_from_slice(&version.2.to_le_bytes());

    // Capability count
    buf.extend_from_slice(&(capabilities.len() as u16).to_le_bytes());

    // SHA-256 digest
    buf.extend_from_slice(&digest);

    // Ed25519 signature (zeros = unsigned)
    buf.extend_from_slice(&[0u8; 64]);

    // Payload length
    buf.extend_from_slice(&(elf_payload.len() as u32).to_le_bytes());

    // Capabilities
    for &cap in capabilities {
        buf.extend_from_slice(&cap.to_le_bytes());
    }

    // ELF payload
    buf.extend_from_slice(elf_payload);

    buf
}

#[cfg(test)]
mod tests {
    // Unit tests would go here for no_std testing harness
}
