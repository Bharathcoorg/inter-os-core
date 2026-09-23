//! ARM Generic Architectural Timer Driver (AArch64)
//!
//! Controls CNTV_CTL_EL0, CNTV_TVAL_EL0, and CNTVCT_EL0 to deliver
//! precise 100 Hz preemptive scheduling ticks.

use crate::hal::traits::PlatformTimer;

pub struct ArmGenericTimer {
    frequency_hz: u64,
}

impl ArmGenericTimer {
    pub const fn new() -> Self {
        Self { frequency_hz: 62_500_000 } // Typical 62.5 MHz ARM timer base
    }

    /// Reads the architectural frequency in Hz from CNTFRQ_EL0.
    pub fn read_frequency(&mut self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let freq: u64;
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
            self.frequency_hz = freq;
            freq
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            self.frequency_hz
        }
    }

    /// Programs the virtual timer to fire an IRQ after `ticks` counter increments.
    pub fn program_countdown(&self, ticks: u32) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            // Write countdown ticks to CNTV_TVAL_EL0
            core::arch::asm!("msr cntv_tval_el0, {}", in(reg) ticks as u64);
            // Enable timer: bit 0 = 1, bit 1 (mask) = 0
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64);
        }
        let _ = ticks;
    }

    /// Reads the raw monotonic virtual counter value (CNTVCT_EL0).
    pub fn read_counter(&self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let count: u64;
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count);
            count
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            crate::hal::TIMER.read_ticks()
        }
    }
}

impl PlatformTimer for ArmGenericTimer {
    fn read_ticks(&self) -> u64 {
        self.read_counter()
    }

    fn frequency_hz(&self) -> u64 {
        self.frequency_hz
    }
}
