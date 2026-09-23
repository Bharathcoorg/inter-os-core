//! LiteVerse Isolated Shared Storage Pool
//!
//! Enforces strict physical/logical isolation between LiteVerse shared network
//! data and private user operating system partitions.
//!
//! Non-Negotiable Principle #16: Private user data must never be mixed with LiteVerse shared storage.

use alloc::string::String;
use alloc::vec::Vec;

/// Errors returned by the LiteVerse storage pool.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiteVerseStorageError {
    /// Attempt to access paths outside `/liteverse/shared/`.
    IsolationViolation,
    /// Storage allocation exceeds user-configured quota.
    QuotaExceeded,
    /// File not found in shared pool.
    NotFound,
}

/// A stored entry in the shared pool.
struct SharedFileEntry {
    path: String,
    data: Vec<u8>,
}

/// Isolated storage pool for LiteVerse network data.
pub struct SharedStoragePool {
    quota_bytes: usize,
    used_bytes: usize,
    entries: Vec<SharedFileEntry>,
}

impl SharedStoragePool {
    pub const SHARED_PREFIX: &'static str = "/liteverse/shared/";

    /// Creates a new storage pool with the given byte quota.
    pub fn new(quota_bytes: usize) -> Self {
        Self {
            quota_bytes,
            used_bytes: 0,
            entries: Vec::new(),
        }
    }

    /// Validates that a target path resides strictly inside the `/liteverse/shared/` domain.
    pub fn validate_path(path: &str) -> Result<(), LiteVerseStorageError> {
        if !path.starts_with(Self::SHARED_PREFIX) {
            return Err(LiteVerseStorageError::IsolationViolation);
        }
        // Disallow path traversal attacks
        if path.contains("..") {
            return Err(LiteVerseStorageError::IsolationViolation);
        }
        Ok(())
    }

    /// Stores a shared network blob within the isolated domain, subject to quota.
    pub fn write_blob(&mut self, path: &str, data: &[u8]) -> Result<(), LiteVerseStorageError> {
        Self::validate_path(path)?;

        let needed = data.len();
        if self.used_bytes + needed > self.quota_bytes {
            return Err(LiteVerseStorageError::QuotaExceeded);
        }

        // Replace if exists, or append
        if let Some(existing) = self.entries.iter_mut().find(|e| e.path == path) {
            self.used_bytes = self.used_bytes.saturating_sub(existing.data.len());
            existing.data = Vec::from(data);
            self.used_bytes = self.used_bytes.saturating_add(needed);
        } else {
            self.entries.push(SharedFileEntry {
                path: String::from(path),
                data: Vec::from(data),
            });
            self.used_bytes = self.used_bytes.saturating_add(needed);
        }

        Ok(())
    }

    /// Reads a shared blob from the isolated pool.
    pub fn read_blob(&self, path: &str) -> Result<&[u8], LiteVerseStorageError> {
        Self::validate_path(path)?;

        self.entries
            .iter()
            .find(|e| e.path == path)
            .map(|e| e.data.as_slice())
            .ok_or(LiteVerseStorageError::NotFound)
    }

    /// Returns current storage utilization in bytes.
    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    /// Returns the maximum allowed storage quota in bytes.
    pub fn quota_bytes(&self) -> usize {
        self.quota_bytes
    }
}
