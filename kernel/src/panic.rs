//! Kernel Panic Handler & Crash Recovery Policy Engine
//!
//! Captures fatal system errors, prints full diagnostic state, and executes
//! an explicit, configurable crash policy (HALT or CONTROLLED REBOOT).

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU8, Ordering};
use crate::serial_println;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PanicPolicy {
    Halt = 0,
    Reboot = 1,
}

impl PanicPolicy {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => PanicPolicy::Reboot,
            _ => PanicPolicy::Halt,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PanicPolicy::Halt => "HALT",
            PanicPolicy::Reboot => "REBOOT",
        }
    }
}

pub static PANIC_POLICY: AtomicU8 = AtomicU8::new(PanicPolicy::Halt as u8);

pub fn set_panic_policy(policy: PanicPolicy) {
    PANIC_POLICY.store(policy as u8, Ordering::SeqCst);
}

pub fn get_panic_policy() -> PanicPolicy {
    PanicPolicy::from_u8(PANIC_POLICY.load(Ordering::SeqCst))
}
