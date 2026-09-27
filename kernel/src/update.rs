//! Secure system-update transaction primitives.
//!
//! Updates are staged as immutable payloads identified by SHA-256 and a
//! publisher signature over a canonical metadata record. Installation policy
//! is monotonic and fail-closed; actual flash/slot I/O belongs to the platform
//! boot/update backend.

use crate::crypto::{ed25519, Sha256, SHA256_DIGEST_LENGTH};
use crate::storage::persistent;
use spin::Mutex;

pub const UPDATE_FORMAT_VERSION: u16 = 1;
pub const UPDATE_SIGNATURE_LEN: usize = ed25519::SIGNATURE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    InvalidFormat,
    Downgrade,
    DigestMismatch,
    InvalidSignature,
    EmptyPayload,
    PersistenceFailed,
    ActivationFailed,
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


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSlot { A, B }

impl UpdateSlot { pub const fn other(self) -> Self { match self { Self::A => Self::B, Self::B => Self::A } } }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivationRecord {
    pub active: UpdateSlot,
    pub pending: Option<UpdateSlot>,
    pub pending_version: u64,
    pub boot_attempts: u8,
}

impl ActivationRecord {
    fn encode(&self) -> [u8; 18] {
        let mut out=[0u8;18];
        out[0]=match self.active { UpdateSlot::A=>0, UpdateSlot::B=>1 };
        out[1]=match self.pending { None=>255, Some(UpdateSlot::A)=>0, Some(UpdateSlot::B)=>1 };
        out[2..10].copy_from_slice(&self.pending_version.to_le_bytes());
        out[10]=self.boot_attempts;
        out
    }
    fn decode(bytes:&[u8]) -> Option<Self> {
        if bytes.len()<11 || bytes[0]>1 || (bytes[1]!=255 && bytes[1]>1) { return None; }
        Some(Self {
            active: if bytes[0]==0 {UpdateSlot::A} else {UpdateSlot::B},
            pending: match bytes[1] { 255=>None, 0=>Some(UpdateSlot::A), _=>Some(UpdateSlot::B) },
            pending_version:u64::from_le_bytes(bytes[2..10].try_into().ok()?),
            boot_attempts:bytes[10],
        })
    }
    fn persist(&self) -> Result<(), UpdateError> { persistent::commit_kind(&self.encode(), 8).map(|_| ()).map_err(|_| UpdateError::PersistenceFailed) }

    pub const fn new(active: UpdateSlot, version: u64) -> Self { Self { active, pending: None, pending_version: version, boot_attempts: 0 } }
    pub fn stage(&mut self, version: u64) -> Result<UpdateSlot, UpdateError> {
        if version <= self.pending_version { return Err(UpdateError::Downgrade); }
        let previous = *self;
        let slot = self.active.other();
        self.pending = Some(slot);
        self.pending_version = version;
        self.boot_attempts = 0;
        if self.persist().is_err() {
            *self = previous;
            return Err(UpdateError::PersistenceFailed);
        }
        Ok(slot)
    }
    pub fn begin_boot(&mut self) -> Result<(), UpdateError> {
        if self.pending.is_some() {
            self.boot_attempts = self.boot_attempts.saturating_add(1);
            self.persist()?;
        }
        Ok(())
    }
    pub fn confirm(&mut self) -> Result<(), UpdateError> {
        if let Some(slot)=self.pending.take() {
            self.active=slot;
            self.boot_attempts=0;
            self.persist()?;
        }
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<(), UpdateError> {
        self.pending=None;
        self.boot_attempts=0;
        self.persist()
    }
    pub const fn boot_attempt_limit(&self) -> u8 { 3 }
    pub const fn recovery_required(&self) -> bool { self.pending.is_some() && self.boot_attempts >= self.boot_attempt_limit() }
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
    fn activation_targets_inactive_slot_and_rolls_back_after_failures() {
        let mut a = ActivationRecord::new(UpdateSlot::A, 10);
        assert_eq!(a.stage(11), Ok(UpdateSlot::B));
        a.begin_boot(); a.begin_boot(); a.begin_boot();
        assert!(a.recovery_required());
        a.rollback();
        assert_eq!(a.active, UpdateSlot::A);
        assert!(a.pending.is_none());
    }

    #[test]
    fn downgrades_are_rejected() {
        let payload = b"update";
        let m = UpdateManifest::new(4, payload.len() as u64, Sha256::digest(payload));
        assert_eq!(UpdateTransaction::stage(m, payload, 4), Err(UpdateError::Downgrade));
    }
}



/// Platform boundary for writing and activating a verified system image.
/// The kernel owns verification policy; board-specific code owns the actual
/// inactive-slot media operations.
pub trait UpdateBackend {
    fn write_inactive_slot(&mut self, slot: UpdateSlot, payload: &[u8]) -> Result<(), UpdateError>;
    fn verify_slot(&mut self, slot: UpdateSlot, manifest: &UpdateManifest) -> Result<(), UpdateError>;
    fn activate_slot(&mut self, slot: UpdateSlot, version: u64) -> Result<(), UpdateError>;
}

pub struct VerifiedUpdate<'a, B: UpdateBackend> { backend: &'a mut B }
impl<'a, B: UpdateBackend> VerifiedUpdate<'a, B> {
    pub fn install(backend:&'a mut B, slot:UpdateSlot, tx:&mut UpdateTransaction, payload:&[u8])->Result<(),UpdateError>{
        if tx.state != UpdateState::Verified { return Err(UpdateError::InvalidFormat); }
        {
            let mut activation = UPDATE_ACTIVATION.lock();
            if slot == activation.active { return Err(UpdateError::ActivationFailed); }
            let previous = *activation;
            activation.pending = Some(slot);
            activation.pending_version = tx.manifest.version;
            activation.boot_attempts = 0;
            if activation.persist().is_err() {
                *activation = previous;
                return Err(UpdateError::PersistenceFailed);
            }
        }
        if let Err(error) = backend.write_inactive_slot(slot,payload)
            .and_then(|_| backend.verify_slot(slot,&tx.manifest))
            .and_then(|_| backend.activate_slot(slot,tx.manifest.version)) {
            let _ = UPDATE_ACTIVATION.lock().rollback();
            return Err(error);
        }
        tx.commit()
    }
}

pub static UPDATE_ACTIVATION: Mutex<ActivationRecord> = Mutex::new(ActivationRecord::new(UpdateSlot::A, 0));

pub fn init() {
    let mut activation = UPDATE_ACTIVATION.lock();
    let mut buf=[0u8;32];
    if let Ok(Some((_,len)))=persistent::load_latest_kind(&mut buf,8) {
        if let Some(restored)=ActivationRecord::decode(&buf[..len]) { *activation=restored; }
    }
}
