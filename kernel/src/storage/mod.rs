//! Storage Subsystem for Inter OS
//!
//! Encompasses raw block storage abstractions, disk partition verifiers,
//! and cryptographic Merkle tree integrity validation.

pub mod verified;
pub mod crashdump;
pub mod vault;

pub use verified::{MerkleVerifier, StorageError, VerifiedBlock, VERIFIED_BLOCK_SIZE};
pub use crashdump::{CrashDumpHeader, CrashDumpManager, CRASH_DUMP, CRASH_DUMP_MAGIC};
pub use vault::{StorageVault, VaultBlock, VaultError, STORAGE_VAULT, VAULT_SECTOR_SIZE};

/// Initializes storage verification, crash dump, and encrypted vault subsystems.
pub fn init() {
    crashdump::init();
    vault::init();
}

