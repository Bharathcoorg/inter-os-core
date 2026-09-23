//! Central State Manager & Attested State Log
//!
//! Tracks operating system state transitions, computes Merkle roots,
//! signs commitments, and maintains an offline-first state log.
//!
//! Non-Negotiable Principle #14: InterLayer is infrastructure, not a requirement
//! for the OS to boot or function offline. State log operates completely offline.

use alloc::vec::Vec;
use crate::crypto::Sha256;
use crate::identity::IdentityVault;
use super::commitment::StateCommitment;
use super::delta::{StateChange, StateDelta};
use super::merkle::compute_merkle_root;

/// Genesis state seed.
pub const GENESIS_STATE_SEED: &[u8] = b"INTER_OS_GENESIS_STATE_ROOT_V0.6";

/// Central State Manager.
pub struct StateManager {
    current_sequence: u64,
    current_state_root: [u8; 32],
    pending_changes: Vec<StateChange>,
    history: Vec<StateCommitment>,
}

impl StateManager {
    /// Creates a new StateManager initialized at Genesis.
    pub fn new() -> Self {
        let genesis_root = Sha256::digest(GENESIS_STATE_SEED);
        Self {
            current_sequence: 0,
            current_state_root: genesis_root,
            pending_changes: Vec::new(),
            history: Vec::new(),
        }
    }

    /// Records a new state transition into the uncommitted delta buffer.
    pub fn record_change(&mut self, change: StateChange) {
        self.pending_changes.push(change);
    }

    /// Returns the number of uncommitted state changes currently buffered.
    pub fn pending_count(&self) -> usize {
        self.pending_changes.len()
    }

    /// Returns the current active 32-byte Merkle state root.
    pub fn current_state_root(&self) -> &[u8; 32] {
        &self.current_state_root
    }

    /// Returns the latest committed sequence number.
    pub fn current_sequence(&self) -> u64 {
        self.current_sequence
    }

    /// Returns the total number of committed state records in the journal.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Commits all buffered pending state transitions into a signed StateCommitment.
    pub fn commit_pending(
        &mut self,
        timestamp_ticks: u64,
        vault: &IdentityVault,
    ) -> Option<StateCommitment> {
        if self.pending_changes.is_empty() {
            return None;
        }

        let new_sequence = self.current_sequence.saturating_add(1);
        let delta = StateDelta::new(
            new_sequence,
            timestamp_ticks,
            core::mem::take(&mut self.pending_changes),
        );

        let leaves = delta.leaf_digests();
        let delta_merkle_root = compute_merkle_root(&leaves);
        let new_state_root = StateCommitment::derive_new_root(
            &self.current_state_root,
            &delta_merkle_root,
        );

        let mut commitment = StateCommitment {
            sequence: new_sequence,
            prev_state_root: self.current_state_root,
            delta_merkle_root,
            new_state_root,
            timestamp_ticks,
            device_did: vault.device_did().clone(),
            signature: [0u8; 32],
        };

        // Sign canonical payload with sovereign device key
        let payload = commitment.canonical_payload();
        commitment.signature = vault.sign(&payload);

        // Advance state chain
        self.current_sequence = new_sequence;
        self.current_state_root = new_state_root;
        self.history.push(commitment.clone());

        Some(commitment)
    }

    /// Verifies the end-to-end cryptographic integrity of the entire state chain.
    pub fn verify_entire_chain(&self, vault: &IdentityVault) -> bool {
        let mut expected_prev = Sha256::digest(GENESIS_STATE_SEED);

        for commitment in &self.history {
            // 1. Verify continuous cryptographic chain link
            if commitment.prev_state_root != expected_prev {
                return false;
            }

            // 2. Verify individual commitment signature and derivation
            if !commitment.verify(vault) {
                return false;
            }

            expected_prev = commitment.new_state_root;
        }

        expected_prev == self.current_state_root
    }
}
