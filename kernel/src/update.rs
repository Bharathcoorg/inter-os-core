//! Secure system-update transaction primitives.
//!
//! Updates are staged as immutable payloads identified by SHA-256 and a
//! publisher signature over a canonical metadata record. Installation policy
//! is monotonic and fail-closed; actual flash/slot I/O belongs to the platform
//! boot/update backend.

use crate::crypto::{ed25519, Sha256, SHA256_DIGEST_LENGTH};

pub const UPDATE_FORMAT_VERSION: u16 = 1;
pub const UPDATE_SIGNATURE_LEN: usize = ed25519::SIGNATURE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    InvalidFormat,
    Downgrade,
    DigestMismatch,
    InvalidSignature,
    EmptyPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState { Idle, Staged, Verified, Committed, Rejected }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateManifest {
    pub format_version: u16,
    pub version: u64,
    pub payload_len: u64,
    pub payload_sha256: [u8; SHA256_DIGEST_LENGTH],
}

impl UpdateManifest {
    pub const fn new(version: u64, payload_len: u64, payload_sha256: [u8; 32]) -> Self {
        Self { format_version: UPDATE_FORMAT_VERSION, version, payload_len, payload_sha256 }
    }

    pub fn validate_payload(&self, payload: &[u8]) -> Result<(), UpdateError> {
        if self.format_version != UPDATE_FORMAT_VERSION || self.payload_len != payload.len() as u64 {
            return Err(UpdateError::InvalidFormat);
        }
        if payload.is_empty() { return Err(UpdateError::EmptyPayload); }
        if Sha256::digest(payload) != self.payload_sha256 { return Err(UpdateError::DigestMismatch); }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> [u8; 50] {
        let mut out = [0u8; 50];
        out[0..2].copy_from_slice(&self.format_version.to_le_bytes());
        out[2..10].copy_from_slice(&self.version.to_le_bytes());
        out[10..18].copy_from_slice(&self.payload_len.to_le_bytes());
        out[18..50].copy_from_slice(&self.payload_sha256);
        out
    }
}

pub struct UpdateTransaction {
    pub manifest: UpdateManifest,
    pub state: UpdateState,
}

impl UpdateTransaction {
    pub fn stage(manifest: UpdateManifest, payload: &[u8], current_version: u64)
        -> Result<Self, UpdateError>
    {
        if manifest.version <= current_version { return Err(UpdateError::Downgrade); }
        manifest.validate_payload(payload)?;
        Ok(Self { manifest, state: UpdateState::Staged })
    }

    pub fn verify_signature(
        &mut self,
        publisher_id: &str,
        signature: &[u8; UPDATE_SIGNATURE_LEN],
    ) -> Result<(), UpdateError> {
        let data = self.manifest.canonical_bytes();
        if !ed25519::verify_detached_signature(&data, publisher_id, signature) {
            self.state = UpdateState::Rejected;
            return Err(UpdateError::InvalidSignature);
        }
        self.state = UpdateState::Verified;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), UpdateError> {
        if self.state != UpdateState::Verified { return Err(UpdateError::InvalidFormat); }
        self.state = UpdateState::Committed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_digest_is_required_before_staging() {
        let payload = b"interos-update";
        let m = UpdateManifest::new(2, payload.len() as u64, Sha256::digest(payload));
        assert!(UpdateTransaction::stage(m, payload, 1).is_ok());
        let bad = UpdateManifest::new(2, payload.len() as u64, [0u8; 32]);
        assert_eq!(UpdateTransaction::stage(bad, payload, 1), Err(UpdateError::DigestMismatch));
    }
    #[test]
    fn downgrades_are_rejected() {
        let payload = b"update";
        let m = UpdateManifest::new(4, payload.len() as u64, Sha256::digest(payload));
        assert_eq!(UpdateTransaction::stage(m, payload, 4), Err(UpdateError::Downgrade));
    }
}