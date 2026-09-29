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

/// Custom panic handler invoked on unexpected kernel failures or assertions.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();

    serial_println!("\n========================================================");
    serial_println!("                 INTEROS CORE KERNEL PANIC                ");
    serial_println!("========================================================");
    if let Some(location) = info.location() {
        serial_println!("Origin:  {}:{}:{}", location.file(), location.line(), location.column());
    } else {
        serial_println!("Origin:  Unknown location");
    }
    serial_println!("Details: {}", info.message());

    // Diagnostic state capture
    let (heap_used, heap_free) = crate::allocator::heap_stats();
    let tasks = if let Some(sched) = crate::task::SCHEDULER.try_lock() {
        sched.task_count()
    } else {
        0
    };
    serial_println!("Diagnostic: heap_used={} heap_free={} tasks={}", heap_used, heap_free, tasks);

    let policy = get_panic_policy();
    match policy {
        PanicPolicy::Halt => {
            serial_println!("Action:  [PANIC POLICY: HALT] CPU halted safely to prevent state corruption.");
            serial_println!("========================================================\n");
            loop {
                x86_64::instructions::hlt();
            }
        }
        PanicPolicy::Reboot => {
            serial_println!("Action:  [PANIC POLICY: REBOOT] Resetting platform for automatic recovery...");
            serial_println!("========================================================\n");
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
            crate::hal::traits::PowerController::reset(&crate::hal::platform::POWER);
        }
    }
}
