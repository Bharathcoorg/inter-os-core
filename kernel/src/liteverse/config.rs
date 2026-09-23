//! LiteVerse Configuration & Participation Quotas
//!
//! Non-Negotiable Principle #15: LiteVerse participation is optional and resource-limited.
//! Non-Negotiable Principle #16: Private user data must never be mixed with LiteVerse shared storage.

/// Configuration parameters governing opt-in LiteVerse network participation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiteVerseConfig {
    /// Explicit user opt-in flag (default: false).
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
    /// Creates a default, safe, opt-in disabled configuration.
    pub const fn default_disabled() -> Self {
        Self {
            enabled: false,
            storage_quota_bytes: 32 * 1024, // 32 KiB default sandbox quota
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
