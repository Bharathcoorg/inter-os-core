//! Boot recovery policy for InterOS.
//!
//! The recovery controller is deliberately storage-agnostic: persistent boot
//! metadata is supplied by the platform/update layer, while this module owns
//! the bounded retry and slot-selection policy.

use spin::Mutex;
use crate::storage::persistent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BootSlot { A = 0, B = 1 }

impl BootSlot {
    pub const fn other(self) -> Self {
        match self { Self::A => Self::B, Self::B => Self::A }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootOutcome { Unknown, Pending, Success, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootRecord {
    pub active: BootSlot,
    pub last_good: BootSlot,
    pub outcome: BootOutcome,
    pub attempts: u8,
    pub max_attempts: u8,
}

impl BootRecord {
    pub const fn new(active: BootSlot) -> Self {
        Self { active, last_good: active, outcome: BootOutcome::Unknown, attempts: 0, max_attempts: 3 }
    }

    pub fn begin(&mut self) {
        self.outcome = BootOutcome::Pending;
        self.attempts = self.attempts.saturating_add(1);
    }

    pub fn success(&mut self) {
        self.last_good = self.active;
        self.outcome = BootOutcome::Success;
        self.attempts = 0;
    }

    pub fn fail(&mut self) {
        self.outcome = BootOutcome::Failed;
    }

    pub const fn should_recover(&self) -> bool {
        matches!(self.outcome, BootOutcome::Failed | BootOutcome::Pending)
            && self.attempts >= self.max_attempts
    }

    pub fn recovery_slot(&self) -> BootSlot {
        if self.should_recover() { self.last_good } else { self.active }
    }

    pub fn switch_to_other(&mut self) {
        self.active = self.active.other();
        self.outcome = BootOutcome::Unknown;
        self.attempts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_slot_recovers_to_last_good_after_bounded_attempts() {
        let mut r = BootRecord::new(BootSlot::A);
        r.switch_to_other();
        r.begin(); r.fail();
        r.begin(); r.fail();
        r.begin(); r.fail();
        assert!(r.should_recover());
        assert_eq!(r.recovery_slot(), BootSlot::A);
    }
}
pub const RECOVERY_RECORD_KIND: u8 = 7;

fn encode(record: &BootRecord) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0] = record.active as u8;
    out[1] = record.last_good as u8;
    out[2] = record.outcome as u8;
    out[3] = record.attempts;
    out[4] = record.max_attempts;
    out
}

fn decode(bytes: &[u8]) -> Option<BootRecord> {
    if bytes.len() < 5 || bytes[0] > 1 || bytes[1] > 1 || bytes[2] > 3 || bytes[4] == 0 { return None; }
    Some(BootRecord {
        active: if bytes[0] == 0 { BootSlot::A } else { BootSlot::B },
        last_good: if bytes[1] == 0 { BootSlot::A } else { BootSlot::B },
        outcome: match bytes[2] { 0 => BootOutcome::Unknown, 1 => BootOutcome::Pending, 2 => BootOutcome::Success, _ => BootOutcome::Failed },
        attempts: bytes[3],
        max_attempts: bytes[4],
    })
}

pub struct BootRecovery {
    pub record: BootRecord,
}

impl BootRecovery {
    pub const fn new() -> Self { Self { record: BootRecord::new(BootSlot::A) } }

    pub fn restore(&mut self) {
        let mut buf = [0u8; 16];
        if let Ok(Some((_, len))) = persistent::load_latest_kind(&mut buf, RECOVERY_RECORD_KIND) {
            if let Some(record) = decode(&buf[..len]) { self.record = record; }
        }
    }

    fn persist(&self) {
        let _ = persistent::commit_kind(&encode(&self.record), RECOVERY_RECORD_KIND);
    }

    pub fn begin_boot(&mut self) { self.record.begin(); self.persist(); }
    pub fn confirm_boot(&mut self) { self.record.success(); self.persist(); }
    pub fn mark_failed(&mut self) { self.record.fail(); self.persist(); }
    pub fn recover_last_good(&mut self) { self.record.active = self.record.last_good; self.record.outcome = BootOutcome::Unknown; self.record.attempts = 0; self.persist(); }
}

pub static BOOT_RECOVERY: Mutex<BootRecovery> = Mutex::new(BootRecovery::new());

pub fn init() {
    let mut recovery = BOOT_RECOVERY.lock();
    recovery.restore();
    if recovery.record.should_recover() {
        recovery.recover_last_good();
    }
    recovery.begin_boot();
}
