//! LiteVerse Optional Decentralized Network Participation Subsystem (AArch64)
//!
//! Non-Negotiable Principle #15: LiteVerse participation is optional and resource-limited.
//! Non-Negotiable Principle #16: Private user data must never be mixed with LiteVerse shared storage.
//! Non-Negotiable Principle #8: Foreground user workloads and battery conservation have absolute priority over LiteVerse.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::crypto::sha256;

/// Operational state of the LiteVerse node.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiteVerseState {
    Disabled,
    Active,
    Throttled,
    Paused,
}

/// Errors returned by the LiteVerse storage and relay engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiteVerseStorageError {
    /// LiteVerse is not enabled by the user (Principle #15).
    Disabled,
    /// Access attempted outside the isolated `/liteverse/shared/` namespace (Principle #16).
    IsolationViolation,
    /// Storage allocation exceeds user-configured quota.
    QuotaExceeded,
    /// Capability token missing or insufficient rights.
    CapabilityDenied,
    /// Battery level below configured minimum threshold for background relay (Principle #8).
    BatteryLow,
    /// File not found in shared pool.
    NotFound,
}

/// Configuration parameters governing opt-in LiteVerse network participation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiteVerseConfig {
    /// Explicit user opt-in flag (default: false, Principle #15).
    pub enabled: bool,
    /// Maximum storage allocation in bytes for shared network data.
    pub storage_quota_bytes: usize,
    /// Maximum bandwidth allocation in bytes per second.
    pub bandwidth_limit_bps: u32,
    /// Minimum battery percentage required to operate (below this, node pauses).
    pub min_battery_percent: u8,
    /// Whether participation is allowed on mobile metered data.
    pub allow_metered_data: bool,
}

impl LiteVerseConfig {
    /// Creates a default, safe, opt-in disabled configuration (Principle #15).
    pub const fn default_disabled() -> Self {
        Self {
            enabled: false,
            storage_quota_bytes: 64 * 1024, // 64 KiB default sandbox quota
            bandwidth_limit_bps: 128 * 1024, // 128 KB/s
            min_battery_percent: 25,
            allow_metered_data: false,
        }
    }

    /// Creates an enabled configuration with specified storage quota.
    pub fn enabled_with_quota(storage_quota_bytes: usize) -> Self {
        Self {
            enabled: true,
            storage_quota_bytes,
            bandwidth_limit_bps: 128 * 1024,
            min_battery_percent: 25,
            allow_metered_data: false,
        }
    }
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
    /// Rejects any path traversal (`..`) or private OS paths (`/vault/`, `/data/`, `/system/`).
    pub fn validate_path(path: &str) -> Result<(), LiteVerseStorageError> {
        if !path.starts_with(Self::SHARED_PREFIX) {
            return Err(LiteVerseStorageError::IsolationViolation);
        }
        if path.contains("..") {
            return Err(LiteVerseStorageError::IsolationViolation);
        }
        Ok(())
    }

    /// Stores a shared network blob within the isolated domain, subject to capability and quota.
    pub fn write_blob(
        &mut self,
        path: &str,
        data: &[u8],
        cap_table: &CapabilityTable,
    ) -> Result<(), LiteVerseStorageError> {
        // Enforce Zero Ambient Authority: require CapabilityType::LiteVerseStorage with Rights::WRITE
        cap_table
            .check(CapabilityType::LiteVerseStorage, Rights::WRITE)
            .map_err(|_| LiteVerseStorageError::CapabilityDenied)?;

        Self::validate_path(path)?;

        let needed = data.len();
        let existing_len = self.entries.iter().find(|e| e.path == path).map(|e| e.data.len()).unwrap_or(0);
        let net_change = needed as isize - existing_len as isize;

        if (self.used_bytes as isize + net_change) > self.quota_bytes as isize {
            return Err(LiteVerseStorageError::QuotaExceeded);
        }

        if let Some(existing) = self.entries.iter_mut().find(|e| e.path == path) {
            self.used_bytes = (self.used_bytes as isize + net_change) as usize;
            existing.data = Vec::from(data);
        } else {
            self.entries.push(SharedFileEntry {
                path: String::from(path),
                data: Vec::from(data),
            });
            self.used_bytes = self.used_bytes.saturating_add(needed);
        }

        Ok(())
    }

    /// Reads a shared blob from the isolated pool, subject to capability.
    pub fn read_blob(
        &self,
        path: &str,
        cap_table: &CapabilityTable,
    ) -> Result<&[u8], LiteVerseStorageError> {
        // Enforce Zero Ambient Authority: require CapabilityType::LiteVerseStorage with Rights::READ
        cap_table
            .check(CapabilityType::LiteVerseStorage, Rights::READ)
            .map_err(|_| LiteVerseStorageError::CapabilityDenied)?;

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

/// Decentralized network participation node.
pub struct LiteVerseNode {
    pub config: LiteVerseConfig,
    pub storage: SharedStoragePool,
    pub blocks_verified: u64,
    pub bytes_relayed: u64,
    pub state: LiteVerseState,
}

impl LiteVerseNode {
    /// Creates a new LiteVerse node instance.
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
        let hash = sha256(block_bytes);
        if hash == *declared_root {
            self.blocks_verified = self.blocks_verified.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Subordinated resource step integrating battery governor and foreground preemption (Principle #8).
    pub fn step(&mut self, battery_percent: u8, is_charging: bool, foreground_active: bool) -> LiteVerseState {
        if !self.config.enabled {
            self.state = LiteVerseState::Disabled;
            return self.state;
        }

        // Foreground user workloads have absolute priority over LiteVerse
        if foreground_active {
            self.state = LiteVerseState::Paused;
            return self.state;
        }

        // Subordinate to battery health: pause if discharging below threshold
        if !is_charging && battery_percent <= self.config.min_battery_percent {
            self.state = LiteVerseState::Paused;
            return self.state;
        }

        self.state = LiteVerseState::Active;
        self.state
    }

    /// Stores a shared relay payload in the isolated pool, enforcing capability and battery thresholds.
    pub fn relay_payload(
        &mut self,
        path: &str,
        data: &[u8],
        cap_table: &CapabilityTable,
        battery_percent: u8,
        is_charging: bool,
    ) -> Result<(), LiteVerseStorageError> {
        if !self.config.enabled {
            return Err(LiteVerseStorageError::Disabled);
        }

        if !is_charging && battery_percent <= self.config.min_battery_percent {
            return Err(LiteVerseStorageError::BatteryLow);
        }

        self.storage.write_blob(path, data, cap_table)?;
        self.bytes_relayed = self.bytes_relayed.saturating_add(data.len() as u64);
        Ok(())
    }
}
