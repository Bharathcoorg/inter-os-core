// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! IRQ-Safe Spinlock Synchronization for AArch64 SMP
//!
//! Masks local CPU IRQs (`DAIF.I`) prior to acquiring the inner spinlock and
//! restores the caller's exact `DAIF` interrupt state when dropped. Prevents
//! IRQ-handler reentrancy deadlocks on `SCHEDULER`, `FRAME_ALLOCATOR`, and
//! hardware-facing state.

use core::ops::{Deref, DerefMut};
use spin::{Mutex, MutexGuard};

#[inline(always)]
fn save_and_disable_irq() -> u64 {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let daif: u64;
        core::arch::asm!(
            "mrs {0}, daif",
            "msr daifset, #2",
            out(reg) daif,
            options(nomem, nostack)
        );
        daif
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

#[inline(always)]
fn restore_irq(saved_daif: u64) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        if (saved_daif & (1 << 7)) == 0 {
            core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = saved_daif;
    }
}

/// Mutual exclusion lock that masks local IRQs while held.
pub struct IrqSpinLock<T> {
    inner: Mutex<T>,
}

impl<T> IrqSpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: Mutex::new(value),
        }
    }

    pub fn lock(&self) -> IrqSpinGuard<'_, T> {
        let saved_daif = save_and_disable_irq();
        let guard = self.inner.lock();
        IrqSpinGuard { guard, saved_daif }
    }

    pub fn try_lock(&self) -> Option<IrqSpinGuard<'_, T>> {
        let saved_daif = save_and_disable_irq();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqSpinGuard { guard, saved_daif }),
            None => {
                restore_irq(saved_daif);
                None
            }
        }
    }
}

pub struct IrqSpinGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    saved_daif: u64,
}

impl<T> Deref for IrqSpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for IrqSpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for IrqSpinGuard<'_, T> {
    fn drop(&mut self) {
        restore_irq(self.saved_daif);
    }
}
