// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Cryptographic Merkle Tree & Verified Storage Layer
//!
//! Enforces dm-verity inspired integrity verification on system partitions.
//! Any block read from disk or rootfs must have its cryptographic SHA-256
//! digest attested against expected Merkle root digests before it is trusted.

use crate::crypto::{Sha256, SHA256_DIGEST_LENGTH};
use crate::drivers::SECTOR_SIZE;
use crate::storage::block::BlockDevice;

/// Verified block size in bytes (matches standard hardware sector size).
pub const VERIFIED_BLOCK_SIZE: usize = SECTOR_SIZE;

/// Maximum Merkle branch proof depth (supports partitions up to 2^16 = 65,536 blocks = 32 MiB).
pub const MAX_MERKLE_PROOF_DEPTH: usize = 16;

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

    /// Validates an in-memory block read with bounds and proof verification.
    pub fn verify_authenticated_read(
        &self,
        block_index: usize,
        total_blocks: usize,
        block_data: &[u8],
        proof: &[[u8; SHA256_DIGEST_LENGTH]],
    ) -> Result<(), StorageError> {
        if block_index >= total_blocks {
            return Err(StorageError::BlockOutOfRange);
        }
        if proof.len() > MAX_MERKLE_PROOF_DEPTH {
            return Err(StorageError::BlockOutOfRange);
        }
        if block_data.len() != VERIFIED_BLOCK_SIZE {
            return Err(StorageError::IntegrityViolation);
        }
        self.verify_branch_proof(block_data, block_index, proof)
    }
}

/// An authenticated block reader connecting physical or virtual block storage
/// to cryptographic Merkle proof verification, ensuring block authentication before use.
pub struct AuthenticatedBlockReader<D> {
    device: D,
    verifier: MerkleVerifier,
    total_blocks: usize,
}

impl<D> AuthenticatedBlockReader<D> {
    /// Creates a new authenticated block reader anchored by a trusted Merkle root.
    pub const fn new(device: D, verifier: MerkleVerifier, total_blocks: usize) -> Self {
        Self {
            device,
            verifier,
            total_blocks,
        }
    }

    /// Access the underlying block device.
    pub fn device(&self) -> &D {
        &self.device
    }

    /// Access the underlying block device mutably.
    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    /// Access the verifier.
    pub fn verifier(&self) -> &MerkleVerifier {
        &self.verifier
    }

    /// Total verified block count.
    pub fn total_blocks(&self) -> usize {
        self.total_blocks
    }
}

impl<D: BlockDevice> AuthenticatedBlockReader<D> {
    /// Reads a block from the underlying block device and cryptographically authenticates it
    /// against the anchored Merkle root before returning the verified data to callers.
    pub fn read_authenticated_block(
        &mut self,
        block_index: usize,
        proof: &[[u8; SHA256_DIGEST_LENGTH]],
        out: &mut [u8; VERIFIED_BLOCK_SIZE],
    ) -> Result<(), StorageError> {
        if block_index >= self.total_blocks {
            return Err(StorageError::BlockOutOfRange);
        }
        if proof.len() > MAX_MERKLE_PROOF_DEPTH {
            return Err(StorageError::BlockOutOfRange);
        }

        self.device
            .read_sector(block_index as u32, out)
            .map_err(|_| StorageError::DeviceError)?;

        self.verifier.verify_branch_proof(out, block_index, proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::block::BlockDeviceError;

    struct MockBlockDevice {
        blocks: [[u8; VERIFIED_BLOCK_SIZE]; 4],
        fail_reads: bool,
    }

    impl BlockDevice for MockBlockDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if self.fail_reads {
                return Err(BlockDeviceError::ReadError);
            }
            if (lba as usize) < self.blocks.len() {
                buf.copy_from_slice(&self.blocks[lba as usize]);
                Ok(())
            } else {
                Err(BlockDeviceError::LbaOutOfRange)
            }
        }

        fn write_sector(&mut self, _lba: u32, _buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            Ok(())
        }

        fn sector_count(&self) -> u32 {
            self.blocks.len() as u32
        }

        fn model(&self) -> &str {
            "MockVerifiedStorage"
        }
    }

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

    #[test]
    fn authenticated_block_reader_full_lifecycle() {
        let mut b0 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b1 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b2 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b3 = [0u8; VERIFIED_BLOCK_SIZE];

        b0[0..12].copy_from_slice(b"RootFsBlock0");
        b1[0..12].copy_from_slice(b"RootFsBlock1");
        b2[0..12].copy_from_slice(b"RootFsBlock2");
        b3[0..12].copy_from_slice(b"RootFsBlock3");

        let h0 = Sha256::digest(&b0);
        let h1 = Sha256::digest(&b1);
        let h2 = Sha256::digest(&b2);
        let h3 = Sha256::digest(&b3);

        let parent0 = MerkleVerifier::combine_hashes(&h0, &h1);
        let parent1 = MerkleVerifier::combine_hashes(&h2, &h3);
        let root = MerkleVerifier::combine_hashes(&parent0, &parent1);

        let verifier = MerkleVerifier::new(root);
        let dev = MockBlockDevice {
            blocks: [b0, b1, b2, b3],
            fail_reads: false,
        };

        let mut reader = AuthenticatedBlockReader::new(dev, verifier, 4);

        // 1. Valid read of block 0
        let mut out = [0u8; VERIFIED_BLOCK_SIZE];
        assert_eq!(reader.read_authenticated_block(0, &[h1, parent1], &mut out), Ok(()));
        assert_eq!(&out[0..12], b"RootFsBlock0");

        // 2. Valid read of block 2
        assert_eq!(reader.read_authenticated_block(2, &[h3, parent0], &mut out), Ok(()));
        assert_eq!(&out[0..12], b"RootFsBlock2");

        // 3. Out of bounds block index
        assert_eq!(
            reader.read_authenticated_block(4, &[h1, parent1], &mut out),
            Err(StorageError::BlockOutOfRange)
        );

        // 4. Proof depth overflow
        let deep_proof = [[0u8; SHA256_DIGEST_LENGTH]; MAX_MERKLE_PROOF_DEPTH + 1];
        assert_eq!(
            reader.read_authenticated_block(0, &deep_proof, &mut out),
            Err(StorageError::BlockOutOfRange)
        );

        // 5. Tampered block on storage device
        reader.device_mut().blocks[0][0] ^= 0x55;
        assert_eq!(
            reader.read_authenticated_block(0, &[h1, parent1], &mut out),
            Err(StorageError::IntegrityViolation)
        );

        // 6. Device I/O failure
        reader.device_mut().fail_reads = true;
        assert_eq!(
            reader.read_authenticated_block(2, &[h3, parent0], &mut out),
            Err(StorageError::DeviceError)
        );
    }
}
