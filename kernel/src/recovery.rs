//! Boot recovery policy for InterOS.
//!
//! The recovery controller is deliberately storage-agnostic: persistent boot
//! metadata is supplied by the platform/update layer, while this module owns
//! the bounded retry and slot-selection policy.

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