//! AMCP (Agent Model Context Protocol) Automation Package Format
//!
//! Enforces verified execution of AI automations according to the core principles:
//! - Package identity and publisher attestation
//! - Cryptographic SHA-256 payload integrity verification
//! - Explicitly declared capability requirements
//! - Deterministic resource quota limits (CPU ticks, RAM)

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::capability::CapabilityType;
use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};

/// Magic header for AMCP automation package format.
pub const AMCP_MAGIC: &[u8; 6] = b"AMCP\x01\x00";

/// Current package format version.
pub const AMCP_VERSION: u16 = 1;

/// Package validation and parsing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageError {
    InvalidMagic,
    UnsupportedVersion,
    BufferTooShort,
    ChecksumMismatch,
    MalformedManifest,
    UnauthorizedCapability,
}

/// Declared resource execution constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_cpu_ticks: u64,
    pub max_memory_bytes: u64,
}

/// Structured AMCP Automation Package.
#[derive(Debug, Clone)]
pub struct AmcpPackage {
    pub package_id: String,
    pub publisher: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub version_patch: u16,
    pub payload_digest: [u8; SHA256_DIGEST_LENGTH],
    pub declared_capabilities: Vec<CapabilityType>,
    pub limits: ResourceLimits,
    pub payload: Vec<u8>,
}

impl AmcpPackage {
    /// Creates a raw serialized binary `.amcp` package.
    pub fn build_binary(
        package_id: &str,
        publisher: &str,
        version: (u16, u16, u16),
        declared_capabilities: &[CapabilityType],
        limits: ResourceLimits,
        payload: &[u8],
    ) -> Vec<u8> {
        let digest = Sha256::digest(payload);
        let mut bytes = Vec::new();

        // 1. Magic Header (6 bytes)
        bytes.extend_from_slice(AMCP_MAGIC);

        // 2. Version (2 bytes)
        bytes.extend_from_slice(&AMCP_VERSION.to_le_bytes());

        // 3. Version tuple (6 bytes)
        bytes.extend_from_slice(&version.0.to_le_bytes());
        bytes.extend_from_slice(&version.1.to_le_bytes());
        bytes.extend_from_slice(&version.2.to_le_bytes());

        // 4. SHA-256 Payload Digest (32 bytes)
        bytes.extend_from_slice(&digest);

        // 5. Resource Limits (16 bytes)
        bytes.extend_from_slice(&limits.max_cpu_ticks.to_le_bytes());
        bytes.extend_from_slice(&limits.max_memory_bytes.to_le_bytes());

        // 6. Declared capabilities count + items
        bytes.push(declared_capabilities.len() as u8);
        for cap in declared_capabilities {
            bytes.push(match cap {
                CapabilityType::SerialOutput => 1,
                CapabilityType::FrameBufferDirect => 2,
                CapabilityType::TimerTickRead => 3,
                CapabilityType::HardwarePortIo => 4,
                CapabilityType::IpcSend => 5,
                CapabilityType::IpcReceive => 6,
                CapabilityType::ProcessSpawn => 7,
                CapabilityType::ProcessKill => 8,
                CapabilityType::FileSystemRead => 9,
                CapabilityType::FileSystemWrite => 10,
                CapabilityType::StorageRawBlock => 11,
                CapabilityType::NetworkSocket => 12,
                CapabilityType::NetworkSend => 13,
                CapabilityType::NetworkReceive => 14,
                CapabilityType::ShmProducer => 15,
                CapabilityType::ShmConsumer => 16,
                CapabilityType::EventPoll => 17,
            });
        }

        // 7. Package ID (length + string)
        let id_bytes = package_id.as_bytes();
        bytes.push(id_bytes.len() as u8);
        bytes.extend_from_slice(id_bytes);

        // 8. Publisher ID (length + string)
        let pub_bytes = publisher.as_bytes();
        bytes.push(pub_bytes.len() as u8);
        bytes.extend_from_slice(pub_bytes);

        // 9. Payload Length (4 bytes) + Payload bytes
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(payload);

        bytes
    }

    /// Parses and cryptographically verifies an `.amcp` package buffer.
    pub fn parse_and_verify(data: &[u8]) -> Result<Self, PackageError> {
        if data.len() < 64 {
            return Err(PackageError::BufferTooShort);
        }

        // 1. Verify Magic
        if &data[0..6] != AMCP_MAGIC {
            return Err(PackageError::InvalidMagic);
        }

        let mut cursor = 6;

        // 2. Format Version
        let fmt_ver = u16::from_le_bytes([data[cursor], data[cursor + 1]]);
        if fmt_ver != AMCP_VERSION {
            return Err(PackageError::UnsupportedVersion);
        }
        cursor += 2;

        // 3. Version tuple
        let v_maj = u16::from_le_bytes([data[cursor], data[cursor + 1]]);
        cursor += 2;
        let v_min = u16::from_le_bytes([data[cursor], data[cursor + 1]]);
        cursor += 2;
        let v_pat = u16::from_le_bytes([data[cursor], data[cursor + 1]]);
        cursor += 2;

        // 4. SHA-256 Digest
        let mut expected_digest = [0u8; 32];
        expected_digest.copy_from_slice(&data[cursor..cursor + 32]);
        cursor += 32;

        // 5. Resource Limits
        let max_ticks = u64::from_le_bytes(data[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let max_mem = u64::from_le_bytes(data[cursor..cursor + 8].try_into().unwrap());
        cursor += 8;
        let limits = ResourceLimits {
            max_cpu_ticks: max_ticks,
            max_memory_bytes: max_mem,
        };

        // 6. Declared capabilities
        let cap_count = data[cursor] as usize;
        cursor += 1;
        let mut declared_capabilities = Vec::with_capacity(cap_count);
        for _ in 0..cap_count {
            let cap_tag = data[cursor];
            cursor += 1;
            let cap = match cap_tag {
                1 => CapabilityType::SerialOutput,
                2 => CapabilityType::FrameBufferDirect,
                3 => CapabilityType::TimerTickRead,
                4 => CapabilityType::HardwarePortIo,
                5 => CapabilityType::IpcSend,
                6 => CapabilityType::IpcReceive,
                7 => CapabilityType::ProcessSpawn,
                8 => CapabilityType::ProcessKill,
                9 => CapabilityType::FileSystemRead,
                10 => CapabilityType::FileSystemWrite,
                11 => CapabilityType::StorageRawBlock,
                12 => CapabilityType::NetworkSocket,
                13 => CapabilityType::NetworkSend,
                14 => CapabilityType::NetworkReceive,
                15 => CapabilityType::ShmProducer,
                16 => CapabilityType::ShmConsumer,
                17 => CapabilityType::EventPoll,
                _ => return Err(PackageError::MalformedManifest),
            };
            declared_capabilities.push(cap);
        }

        // 7. Package ID
        let id_len = data[cursor] as usize;
        cursor += 1;
        let id_bytes = &data[cursor..cursor + id_len];
        let package_id = core::str::from_utf8(id_bytes)
            .map_err(|_| PackageError::MalformedManifest)?
            .to_string();
        cursor += id_len;

        // 8. Publisher ID
        let pub_len = data[cursor] as usize;
        cursor += 1;
        let pub_bytes = &data[cursor..cursor + pub_len];
        let publisher = core::str::from_utf8(pub_bytes)
            .map_err(|_| PackageError::MalformedManifest)?
            .to_string();
        cursor += pub_len;

        // 9. Payload
        let payload_len = u32::from_le_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let payload = data[cursor..cursor + payload_len].to_vec();

        // 10. Cryptographic Attestation Check
        let actual_digest = Sha256::digest(&payload);
        if actual_digest != expected_digest {
            return Err(PackageError::ChecksumMismatch);
        }

        Ok(Self {
            package_id,
            publisher,
            version_major: v_maj,
            version_minor: v_min,
            version_patch: v_pat,
            payload_digest: expected_digest,
            declared_capabilities,
            limits,
            payload,
        })
    }

    /// Computes the developer cryptographic signature (HMAC-SHA256) over package metadata.
    pub fn compute_signature(
        package_id: &str,
        version: (u16, u16, u16),
        payload_digest: &[u8; 32],
        developer_key: &[u8],
    ) -> [u8; 32] {
        let mut msg = Vec::new();
        msg.extend_from_slice(package_id.as_bytes());
        msg.extend_from_slice(&version.0.to_le_bytes());
        msg.extend_from_slice(&version.1.to_le_bytes());
        msg.extend_from_slice(&version.2.to_le_bytes());
        msg.extend_from_slice(payload_digest);
        crate::crypto::hmac::HmacSha256::mac(developer_key, &msg)
    }

    /// Verifies the developer cryptographic signature against the expected tag.
    pub fn verify_signature(&self, developer_key: &[u8], expected_signature: &[u8; 32]) -> bool {
        let actual = Self::compute_signature(
            &self.package_id,
            (self.version_major, self.version_minor, self.version_patch),
            &self.payload_digest,
            developer_key,
        );
        let mut diff = 0u8;
        for i in 0..32 {
            diff |= actual[i] ^ expected_signature[i];
        }
        diff == 0
    }

    /// Pre-flight risk assessment evaluating declared capabilities against security policy tiers.
    pub fn assess_risk(&self) -> CapabilityRisk {
        for cap in &self.declared_capabilities {
            match cap {
                CapabilityType::StorageRawBlock
                | CapabilityType::HardwarePortIo
                | CapabilityType::ProcessKill
                | CapabilityType::ProcessSpawn => return CapabilityRisk::Forbidden,

                CapabilityType::NetworkSocket
                | CapabilityType::NetworkSend
                | CapabilityType::NetworkReceive
                | CapabilityType::FileSystemWrite => return CapabilityRisk::RequiresConsent,

                _ => {}
            }
        }
        CapabilityRisk::Safe
    }
}

/// Security risk assessment category for marketplace automation packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityRisk {
    /// Safe operations (Serial log, Timer tick, FileSystem read); permitted by default.
    Safe = 0,
    /// Operations impacting network or filesystem; requires user capability consent token.
    RequiresConsent = 1,
    /// High-privilege hardware operations strictly forbidden for third-party marketplace packages.
    Forbidden = 2,
}
