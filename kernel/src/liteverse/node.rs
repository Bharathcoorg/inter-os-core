//! LiteVerse Decentralized Node Worker
//!
//! Handles background block verification, peer relaying, and quota auditing.
//! Subordinates all activity to foreground user applications and hardware thermals.

use crate::crypto::Sha256;
use super::config::LiteVerseConfig;
use super::storage::{LiteVerseStorageError, SharedStoragePool};

/// Operational state of the LiteVerse node.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiteVerseState {
    Disabled,
    Active,
    Throttled,
    Paused,
}

/// Decentralized network participation node.
pub struct LiteVerseNode {
    pub config: LiteVerseConfig,
    pub storage: SharedStoragePool,
    pub blocks_verified: u64,
    pub bytes_relayed: u64,
    pub state: LiteVerseState,
}

impl LiteVerseNode {
    /// Creates a new LiteVerse node.
    pub fn new(config: LiteVerseConfig) -> Self {
        let quota = config.storage_quota_bytes;
        let initial_state = if config.enabled {
            LiteVerseState::Active
        } else {
            LiteVerseState::Disabled
        };

        Self {
            config,
            storage: SharedStoragePool::new(quota),
            blocks_verified: 0,
            bytes_relayed: 0,
            state: initial_state,
        }
    }

    /// Verifies a distributed block candidate against its declared Merkle root.
    pub fn verify_block(&mut self, block_bytes: &[u8], declared_root: &[u8; 32]) -> bool {
        let hash = Sha256::digest(block_bytes);
        if hash == *declared_root {
            self.blocks_verified = self.blocks_verified.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Performs one worker step, strictly respecting resource permission.
    pub fn step(&mut self, resource_permitted: bool) -> LiteVerseState {
        if !self.config.enabled {
            self.state = LiteVerseState::Disabled;
            return self.state;
        }

        if !resource_permitted {
            self.state = LiteVerseState::Paused;
            return self.state;
        }

        self.state = LiteVerseState::Active;
        self.state
    }

    /// Stores a shared relay payload in the isolated pool.
    pub fn relay_payload(&mut self, path: &str, data: &[u8]) -> Result<(), LiteVerseStorageError> {
        self.storage.write_blob(path, data)?;
        self.bytes_relayed = self.bytes_relayed.saturating_add(data.len() as u64);
        Ok(())
    }
}
