// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

    /// Verifies a cryptographic Merkle branch proof from a leaf block to the anchor root digest.
    pub fn verify_branch_proof(
        &self,
        leaf_data: &[u8],
        leaf_index: usize,
        audit_path: &[[u8; SHA256_DIGEST_LENGTH]],
    ) -> Result<(), StorageError> {
        let mut current_hash = Sha256::digest(leaf_data);
        let mut idx = leaf_index;
        for sibling in audit_path {
            if idx % 2 == 0 {
                current_hash = Self::combine_hashes(&current_hash, sibling);
            } else {
                current_hash = Self::combine_hashes(sibling, &current_hash);
            }
            idx /= 2;
        }
        if current_hash == self.root_digest {
            Ok(())
        } else {
            Err(StorageError::IntegrityViolation)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merkle_branch_proof_valid_and_tampered() {
        let leaf0 = b"Partition block 0 content";
        let leaf1 = b"Partition block 1 content";
        let leaf2 = b"Partition block 2 content";
        let leaf3 = b"Partition block 3 content";

        let h0 = Sha256::digest(leaf0);
        let h1 = Sha256::digest(leaf1);
        let h2 = Sha256::digest(leaf2);
        let h3 = Sha256::digest(leaf3);

        let parent0 = MerkleVerifier::combine_hashes(&h0, &h1);
        let parent1 = MerkleVerifier::combine_hashes(&h2, &h3);
        let root = MerkleVerifier::combine_hashes(&parent0, &parent1);

        let verifier = MerkleVerifier::new(root);

        // Valid proof for leaf 0: audit path = [h1, parent1]
        assert_eq!(verifier.verify_branch_proof(leaf0, 0, &[h1, parent1]), Ok(()));

        // Valid proof for leaf 2: audit path = [h3, parent0]
        assert_eq!(verifier.verify_branch_proof(leaf2, 2, &[h3, parent0]), Ok(()));

        // Tampered leaf data must be rejected
        let tampered_leaf = b"Partition block 0 TAMPERED";
        assert_eq!(
            verifier.verify_branch_proof(tampered_leaf, 0, &[h1, parent1]),
            Err(StorageError::IntegrityViolation)
        );

        // Tampered sibling in audit path must be rejected
        let mut tampered_h1 = h1;
        tampered_h1[0] ^= 0xFF;
        assert_eq!(
            verifier.verify_branch_proof(leaf0, 0, &[tampered_h1, parent1]),
            Err(StorageError::IntegrityViolation)
        );
    }
}
