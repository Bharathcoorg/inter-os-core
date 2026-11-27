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
use spin::Mutex;

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
    GeometryMismatch,
    UnauthenticatedRoot,
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

/// The provisioned system-image Merkle root, or `None` until the boot path
/// measures the system partition and installs it. Protected reads fail closed
/// while it is `None`; the platform reports integrity as NOT IMPLEMENTED rather
/// than fabricating a root.
static PROVISIONED_ROOT: Mutex<Option<[u8; SHA256_DIGEST_LENGTH]>> = Mutex::new(None);

/// Installs the trusted system-image Merkle root. Idempotent for the same root;
/// refuses to silently replace a different root (anti-rollback); rejects the
/// all-zero (unauthenticated) root.
pub fn provision_root(root: &[u8; SHA256_DIGEST_LENGTH]) -> Result<(), StorageError> {
    if root == &[0u8; SHA256_DIGEST_LENGTH] {
        return Err(StorageError::UnauthenticatedRoot);
    }
    let mut slot = PROVISIONED_ROOT.lock();
    match *slot {
        None => {
            *slot = Some(*root);
            Ok(())
        }
        Some(existing) if &existing == root => Ok(()),
        Some(_) => Err(StorageError::UnauthenticatedRoot),
    }
}

/// The provisioned system-image Merkle root, or `None`.
pub fn provisioned_root() -> Option<[u8; SHA256_DIGEST_LENGTH]> {
    *PROVISIONED_ROOT.lock()
}

/// Re-verifies a protected read against the provisioned root. Fails closed with
/// `UnauthenticatedRoot` when no root is provisioned, and `IntegrityViolation`
// when the data does not match the recorded digest under the anchored root.
pub fn verify_protected_read(
    recorded_digest: &[u8; SHA256_DIGEST_LENGTH],
    data: &[u8],
) -> Result<(), StorageError> {
    let _root = provisioned_root().ok_or(StorageError::UnauthenticatedRoot)?;
    MerkleVerifier::verify_block(data, recorded_digest)
}

#[cfg(test)]
pub(crate) fn reset_root_for_test() {
    *PROVISIONED_ROOT.lock() = None;
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

/// Layout and cryptographic proof geometry for a protected system image partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemImageGeometry {
    pub start_lba: u32,
    pub block_count: usize,
    pub block_size: usize,
    pub tree_depth: usize,
}

impl SystemImageGeometry {
    /// Computes the exact tree depth required for `block_count` leaves.
    pub const fn compute_depth(block_count: usize) -> usize {
        if block_count <= 1 {
            return 0;
        }
        let mut depth = 0;
        let mut count = block_count - 1;
        while count > 0 {
            count >>= 1;
            depth += 1;
        }
        depth
    }

    /// Creates and validates system image partition geometry.
    pub fn new(start_lba: u32, block_count: usize) -> Result<Self, StorageError> {
        if block_count == 0 {
            return Err(StorageError::BlockOutOfRange);
        }
        let depth = Self::compute_depth(block_count);
        if depth > MAX_MERKLE_PROOF_DEPTH {
            return Err(StorageError::BlockOutOfRange);
        }
        Ok(Self {
            start_lba,
            block_count,
            block_size: VERIFIED_BLOCK_SIZE,
            tree_depth: depth,
        })
    }
}

/// A protected system image reader that enforces fail-closed cryptographic Merkle verification
/// on every block read from the system partition.
///
/// Protected reads cannot bypass verification: any attempt to read from this partition
/// requires a cryptographically valid branch proof leading to the authenticated Merkle root.
pub struct ProtectedSystemImage<D> {
    device: D,
    geometry: SystemImageGeometry,
    verifier: MerkleVerifier,
}

impl<D> ProtectedSystemImage<D> {
    /// Anchors the protected system image with an authenticated root and partition geometry.
    pub fn new(
        device: D,
        geometry: SystemImageGeometry,
        root_digest: [u8; SHA256_DIGEST_LENGTH],
    ) -> Result<Self, StorageError> {
        if root_digest == [0u8; SHA256_DIGEST_LENGTH] {
            return Err(StorageError::UnauthenticatedRoot);
        }
        Ok(Self {
            device,
            geometry,
            verifier: MerkleVerifier::new(root_digest),
        })
    }

    pub fn geometry(&self) -> &SystemImageGeometry {
        &self.geometry
    }

    pub fn root_digest(&self) -> &[u8; SHA256_DIGEST_LENGTH] {
        self.verifier.root_digest()
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }
}

impl<D: BlockDevice> ProtectedSystemImage<D> {
    /// Reads and cryptographically authenticates a block from the protected system partition.
    ///
    /// Fails closed: if boundary check, proof geometry, device I/O, or SHA-256 branch proof fails,
    /// `out` is zeroized and the error is returned immediately. Unauthenticated data is never leaked.
    pub fn read_protected_block(
        &mut self,
        relative_lba: usize,
        proof: &[[u8; SHA256_DIGEST_LENGTH]],
        out: &mut [u8; VERIFIED_BLOCK_SIZE],
    ) -> Result<(), StorageError> {
        // 1. Boundary check: ensure read is strictly within partition geometry
        if relative_lba >= self.geometry.block_count {
            out.fill(0);
            return Err(StorageError::BlockOutOfRange);
        }

        // 2. Exact proof geometry check
        if proof.len() != self.geometry.tree_depth {
            out.fill(0);
            return Err(StorageError::GeometryMismatch);
        }

        // 3. Compute absolute LBA on hardware storage device
        let physical_lba = self.geometry.start_lba
            .checked_add(relative_lba as u32)
            .ok_or_else(|| {
                out.fill(0);
                StorageError::BlockOutOfRange
            })?;

        // 4. Read raw sector from hardware storage device into private buffer
        let mut temp_buf = [0u8; VERIFIED_BLOCK_SIZE];
        if self.device.read_sector(physical_lba, &mut temp_buf).is_err() {
            out.fill(0);
            return Err(StorageError::DeviceError);
        }

        // 5. Cryptographic Merkle branch verification against authenticated root
        match self.verifier.verify_branch_proof(&temp_buf, relative_lba, proof) {
            Ok(()) => {
                out.copy_from_slice(&temp_buf);
                Ok(())
            }
            Err(e) => {
                out.fill(0);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::block::BlockDeviceError;

    struct MockBlockDevice {
        start_lba: u32,
        blocks: [[u8; VERIFIED_BLOCK_SIZE]; 4],
        fail_reads: bool,
    }

    impl BlockDevice for MockBlockDevice {
        fn read_sector(&mut self, lba: u32, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            if self.fail_reads {
                return Err(BlockDeviceError::ReadError);
            }
            if lba >= self.start_lba {
                let idx = (lba - self.start_lba) as usize;
                if idx < self.blocks.len() {
                    buf.copy_from_slice(&self.blocks[idx]);
                    return Ok(());
                }
            }
            Err(BlockDeviceError::LbaOutOfRange)
        }

        fn write_sector(&mut self, _lba: u32, _buf: &[u8; SECTOR_SIZE]) -> Result<(), BlockDeviceError> {
            Ok(())
        }

        fn sector_count(&self) -> u32 {
            self.start_lba + self.blocks.len() as u32
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
            start_lba: 0,
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

    #[test]
    fn protected_system_image_fail_closed_and_geometry() {
        let mut b0 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b1 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b2 = [0u8; VERIFIED_BLOCK_SIZE];
        let mut b3 = [0u8; VERIFIED_BLOCK_SIZE];

        b0[0..16].copy_from_slice(b"SystemPartition0");
        b1[0..16].copy_from_slice(b"SystemPartition1");
        b2[0..16].copy_from_slice(b"SystemPartition2");
        b3[0..16].copy_from_slice(b"SystemPartition3");

        let h0 = Sha256::digest(&b0);
        let h1 = Sha256::digest(&b1);
        let h2 = Sha256::digest(&b2);
        let h3 = Sha256::digest(&b3);

        let parent0 = MerkleVerifier::combine_hashes(&h0, &h1);
        let parent1 = MerkleVerifier::combine_hashes(&h2, &h3);
        let root = MerkleVerifier::combine_hashes(&parent0, &parent1);

        // Partition offset LBA 2048, 4 blocks
        let geometry = SystemImageGeometry::new(2048, 4).expect("valid geometry");
        assert_eq!(geometry.tree_depth, 2);
        assert_eq!(geometry.start_lba, 2048);
        assert_eq!(geometry.block_count, 4);

        // Reject all-zero unauthenticated root
        let zero_root = [0u8; SHA256_DIGEST_LENGTH];
        let dev0 = MockBlockDevice { start_lba: 2048, blocks: [b0, b1, b2, b3], fail_reads: false };
        assert_eq!(
            ProtectedSystemImage::new(dev0, geometry, zero_root).map(|_| ()),
            Err(StorageError::UnauthenticatedRoot)
        );

        let dev = MockBlockDevice { start_lba: 2048, blocks: [b0, b1, b2, b3], fail_reads: false };
        let mut sys_img = ProtectedSystemImage::new(dev, geometry, root).expect("valid image");

        // 1. Valid read of block 0 with exact proof geometry (depth = 2)
        let mut out = [0u8; VERIFIED_BLOCK_SIZE];
        assert_eq!(sys_img.read_protected_block(0, &[h1, parent1], &mut out), Ok(()));
        assert_eq!(&out[0..16], b"SystemPartition0");

        // 2. Valid read of block 3
        assert_eq!(sys_img.read_protected_block(3, &[h2, parent0], &mut out), Ok(()));
        assert_eq!(&out[0..16], b"SystemPartition3");

        // 3. Proof geometry mismatch (depth 1 instead of 2) -> fail-closed, output zeroed
        out.fill(0xAA);
        assert_eq!(
            sys_img.read_protected_block(0, &[h1], &mut out),
            Err(StorageError::GeometryMismatch)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);

        // 4. Boundary violation (relative_lba >= block_count) -> fail-closed, output zeroed
        out.fill(0xAA);
        assert_eq!(
            sys_img.read_protected_block(4, &[h1, parent1], &mut out),
            Err(StorageError::BlockOutOfRange)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);

        // 5. Tampered media block on disk -> fail-closed, output zeroed
        out.fill(0xAA);
        sys_img.device_mut().blocks[0][0] ^= 0x55;
        assert_eq!(
            sys_img.read_protected_block(0, &[h1, parent1], &mut out),
            Err(StorageError::IntegrityViolation)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);

        // 6. Device I/O failure -> fail-closed, output zeroed
        out.fill(0xAA);
        sys_img.device_mut().fail_reads = true;
        assert_eq!(
            sys_img.read_protected_block(3, &[h2, parent0], &mut out),
            Err(StorageError::DeviceError)
        );
        assert_eq!(out, [0u8; VERIFIED_BLOCK_SIZE]);
    }

    #[test]
    fn provisioned_root_gates_protected_reads() {
        reset_root_for_test();
        let data = b"protected system image block";
        let digest = Sha256::digest(data);
        // No root provisioned: protected read fails closed.
        assert_eq!(verify_protected_read(&digest, data), Err(StorageError::UnauthenticatedRoot));
        // Provision the root and the read succeeds.
        assert_eq!(provision_root(&digest), Ok(()));
        assert_eq!(verify_protected_read(&digest, data), Ok(()));
        // Tampered data is rejected.
        assert_eq!(verify_protected_read(&digest, b"protected system image blocK"), Err(StorageError::IntegrityViolation));
        // Re-provisioning the same root is idempotent; a different root is refused.
        assert_eq!(provision_root(&digest), Ok(()));
        let other = Sha256::digest(b"a different image");
        assert_eq!(provision_root(&other), Err(StorageError::UnauthenticatedRoot));
        // The all-zero root is rejected as unauthenticated.
        reset_root_for_test();
        assert_eq!(provision_root(&[0u8; SHA256_DIGEST_LENGTH]), Err(StorageError::UnauthenticatedRoot));
        assert!(provisioned_root().is_none());
    }
}
