//! Persistent storage subsystem for InterOS.
pub mod verified;
pub mod crashdump;
pub mod vault;
pub mod persistent;

pub use verified::{MerkleVerifier, StorageError, VerifiedBlock, VERIFIED_BLOCK_SIZE};
pub use crashdump::{CrashDumpHeader, CrashDumpManager, CRASH_DUMP, CRASH_DUMP_MAGIC};
pub use vault::{StorageVault, VaultBlock, VaultError, STORAGE_VAULT, VAULT_SECTOR_SIZE};

/// Initializes storage verification, crash dump, encrypted vault, and the
/// crash-consistent metadata journal.
pub fn init() {
    crashdump::init();
    vault::init();
}
