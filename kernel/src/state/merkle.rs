//! Cryptographic Merkle State Tree
//!
//! Generates deterministic 32-byte Merkle root hashes over state delta batches
//! to provide compact, tamper-evident cryptographic commitments.

use alloc::vec::Vec;
use crate::crypto::Sha256;

/// Computes the 32-byte Merkle root hash for a given list of leaf digests.
pub fn compute_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    if leaves.len() == 1 {
        return leaves[0];
    }

    let mut current_layer: Vec<[u8; 32]> = leaves.to_vec();

    while current_layer.len() > 1 {
        let mut next_layer = Vec::with_capacity((current_layer.len() + 1) / 2);

        for chunk in current_layer.chunks(2) {
            let left = &chunk[0];
            let right = if chunk.len() > 1 {
                &chunk[1]
            } else {
                // If odd number of nodes, duplicate the last node
                &chunk[0]
            };

            let mut combined = [0u8; 64];
            combined[..32].copy_from_slice(left);
            combined[32..].copy_from_slice(right);
            next_layer.push(Sha256::digest(&combined));
        }

        current_layer = next_layer;
    }

    current_layer[0]
}
