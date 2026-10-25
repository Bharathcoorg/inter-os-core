//! Preemption-safe kernel locking.
//!
//! InterOS runs on one CPU. Syscall bodies are preemptible (see
//! `syscall::entry`), so the timer interrupt can switch a task out at any
//! instruction, including inside a spinlock critical section. If the scheduler
//! then marks that task blocked — the background-budget path does exactly that
//! — the lock stays held by a task that is not runnable, and every other task
//! spins on it forever.
//!
//! `Mutex` wraps `spin::Mutex` and defers scheduler preemption while the guard
//! lives. The timer still fires and still accounts ticks and events; only the
//! context switch is postponed until the critical section ends. Locks that
//! interrupt handlers acquire on the tick path must keep using `try_lock`.
//!
//! This does not mask hardware interrupts: a long critical section (for example
//! a VFS disk commit) still lets the UART and input IRQs be serviced.

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

static PREEMPT_COUNT: AtomicU32 = AtomicU32::new(0);

/// Defers scheduler preemption until the returned guard is dropped.
#[derive(Debug)]
pub struct PreemptGuard(());

impl PreemptGuard {
    pub fn new() -> Self {
        PREEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
        Self(())
    }
}

impl Default for PreemptGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PreemptGuard {
    fn drop(&mut self) {
        PREEMPT_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Whether the current task holds a lock whose critical section must not be
/// interrupted by a context switch.
#[inline]
pub fn preemption_disabled() -> bool {
    PREEMPT_COUNT.load(Ordering::Relaxed) != 0
}

/// Spin mutex whose critical section is protected from scheduler preemption.
#[derive(Debug)]
pub struct Mutex<T: ?Sized> {
    inner: spin::Mutex<T>,
}

/// Guard returned by [`Mutex::lock`] and [`Mutex::try_lock`].
#[derive(Debug)]
pub struct MutexGuard<'a, T: ?Sized> {
    inner: spin::MutexGuard<'a, T>,
    // Declared last so the lock is released before preemption is restored.
    _preempt: PreemptGuard,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: spin::Mutex::new(value) }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        let preempt = PreemptGuard::new();
        MutexGuard { inner: self.inner.lock(), _preempt: preempt }
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let preempt = PreemptGuard::new();
        match self.inner.try_lock() {
            Some(inner) => Some(MutexGuard { inner, _preempt: preempt }),
            None => {
                drop(preempt);
                None
            }
        }
    }
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_defers_and_restores_preemption() {
        let m = Mutex::new(0u32);
        assert!(!preemption_disabled());
        {
            let mut g = m.lock();
            assert!(preemption_disabled());
            *g = 7;
            {
                let inner = m.try_lock();
                assert!(inner.is_none(), "a held lock must refuse try_lock");
            }
        }
        assert!(!preemption_disabled());
        assert_eq!(*m.lock(), 7);
    }

    #[test]
    fn failed_try_lock_does_not_leak_preempt_state() {
        let m = Mutex::new(());
        let g = m.lock();
        assert!(m.try_lock().is_none());
        assert!(preemption_disabled());
        drop(g);
        assert!(!preemption_disabled());
    }
}
