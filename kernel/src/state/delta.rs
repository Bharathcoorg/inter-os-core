//! State Delta & Domain Change Representation
//!
//! Models incremental state transitions across the operating system.
//! Non-Negotiable Principle #17: Private state remains encrypted/local; state deltas
//! are represented as verifiable, tamper-evident cryptographic log entries.

use alloc::string::String;
use alloc::vec::Vec;
use crate::crypto::Sha256;

/// Subsystem domain for a state change.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StateDomain {
    /// System and user configuration settings.
    Settings = 1,
    /// Dynamic capability table grants and revocations.
    CapabilityGrants = 2,
    /// Registered AMCP automation packages and policies.
    AutomationProfiles = 3,
    /// Sandboxed application persistent metadata.
    ApplicationState = 4,
    /// Resource management quotas and operational limits.
    ResourceQuotas = 5,
}

/// An atomic state transition within a domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateChange {
    pub domain: StateDomain,
    pub key: String,
    pub value: Vec<u8>,
}

impl StateChange {
    /// Creates a new state change entry.
    pub fn new(domain: StateDomain, key: &str, value: &[u8]) -> Self {
        Self {
            domain,
            key: String::from(key),
            value: Vec::from(value),
        }
    }

    /// Computes the 32-byte cryptographic digest of this individual state change.
    pub fn digest(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(1 + self.key.len() + self.value.len());
        buf.push(self.domain as u8);
        buf.extend_from_slice(self.key.as_bytes());
        buf.extend_from_slice(&self.value);
        Sha256::digest(&buf)
    }
}

/// A batch of state changes forming an incremental state delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateDelta {
    pub sequence: u64,
    pub timestamp_ticks: u64,
    pub changes: Vec<StateChange>,
}

impl StateDelta {
    /// Creates a new state delta.
    pub fn new(sequence: u64, timestamp_ticks: u64, changes: Vec<StateChange>) -> Self {
        Self {
            sequence,
            timestamp_ticks,
            changes,
        }
    }

    /// Returns the digests of all individual changes in this delta.
    pub fn leaf_digests(&self) -> Vec<[u8; 32]> {
        self.changes.iter().map(|c| c.digest()).collect()
    }
}
