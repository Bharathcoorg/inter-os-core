//! Cryptographic Merkle Tree & Verified Storage Layer
//!
//! Enforces dm-verity inspired integrity verification on system partitions.
//! Any block read from disk or rootfs must have its cryptographic SHA-256
//! digest attested against expected Merkle root digests before it is trusted.

use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::drivers::SECTOR_SIZE;

/// Verified block size in bytes (matches standard hardware sector size).
pub const VERIFIED_BLOCK_SIZE: usize = SECTOR_SIZE;

/// Verification and storage error states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    IntegrityViolation,
    BlockOutOfRange,
    DeviceError,
    TamperedData,
}

/// Cryptographically attested block descriptor.
#[derive(Debug, Clone)]
pub struct VerifiedBlock {
    pub lba: u32,
    pub expected_hash: [u8; SHA256_DIGEST_LENGTH],
}

/// In-kernel Merkle rootfs verifier.
pub struct MerkleVerifier {
    root_digest: [u8; SHA256_DIGEST_LENGTH],
}

impl MerkleVerifier {
    /// Creates a new verifier anchored by an authorized Merkle root hash.
    pub const fn new(root_digest: [u8; SHA256_DIGEST_LENGTH]) -> Self {
        Self { root_digest }
    }

    /// Verifies a data block against its expected SHA-256 digest.
    pub fn verify_block(data: &[u8], expected_digest: &[u8; SHA256_DIGEST_LENGTH]) -> Result<(), StorageError> {
        let actual_digest = Sha256::digest(data);
        if &actual_digest == expected_digest {
            Ok(())
        } else {
            Err(StorageError::IntegrityViolation)
        }
    }

    /// Returns the active Merkle root digest.
    pub fn root_digest(&self) -> &[u8; SHA256_DIGEST_LENGTH] {
        &self.root_digest
    }

    /// Computes parent hash of two child Merkle nodes: SHA256(left || right).
    pub fn combine_hashes(
        left: &[u8; SHA256_DIGEST_LENGTH],
        right: &[u8; SHA256_DIGEST_LENGTH],
    ) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut hasher = Sha256::new();
        hasher.update(left);
        hasher.update(right);
        hasher.finalize()
    }
}
