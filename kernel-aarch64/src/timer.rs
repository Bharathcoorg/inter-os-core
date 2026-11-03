// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! ARM Generic Architectural Timer Driver (AArch64)
//!
//! Controls CNTFRQ_EL0, CNTV_CTL_EL0, CNTV_TVAL_EL0, and CNTVCT_EL0
//! to provide precise microsecond-grade monotonic time and 100 Hz ticks.

pub struct ArmGenericTimer {
    pub frequency_hz: u64,
}

impl ArmGenericTimer {
    pub const fn new() -> Self {
        Self { frequency_hz: 62_500_000 }
    }

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

    pub fn program_countdown(&self, ticks: u32) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("msr cntv_tval_el0, {}", in(reg) ticks as u64);
            core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64);
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            let _ = ticks;
        }
    }

    pub fn read_counter(&self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let count: u64;
            core::arch::asm!("mrs {}, cntvct_el0", out(reg) count);
            count
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            0
        }
    }
}
