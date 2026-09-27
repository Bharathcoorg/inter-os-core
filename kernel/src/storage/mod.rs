//! Persistent storage subsystem for InterOS.
pub mod verified;
pub mod crashdump;
pub mod vault;
pub mod persistent;
pub mod interfs;
pub mod filesystem;
pub mod encrypted;
pub mod block;
pub mod volume;

pub use block::{BlockDevice, BlockDeviceError, BlockDeviceInfo, BlockDeviceKind, SECTOR_SIZE};
pub use volume::{VolumeDescriptor, VolumeError, VolumeKind, VolumeTable, MAX_VOLUMES};
pub use interfs::{InterFs, FsBackendError, FS_MAGIC, FS_VERSION};
pub use filesystem::{BLOCK_FS_BASE_LBA, BLOCK_FS_TAIL_RESERVED, init_block_filesystem, load_data_file, store_data_file};
pub use encrypted::{seal, open, EncryptionError, ENCRYPTED_MAGIC, ENCRYPTED_VERSION, ENCRYPTED_OVERHEAD};

pub use verified::{MerkleVerifier, StorageError, VerifiedBlock, VERIFIED_BLOCK_SIZE};
pub use crashdump::{CrashDumpHeader, CrashDumpManager, CRASH_DUMP, CRASH_DUMP_MAGIC};
pub use vault::{StorageVault, VaultBlock, VaultError, STORAGE_VAULT, VAULT_SECTOR_SIZE};

/// Initializes storage verification, crash dump, encrypted vault, and the
/// crash-consistent metadata journal.
pub fn init() {
    crashdump::init();
    vault::init();
}
