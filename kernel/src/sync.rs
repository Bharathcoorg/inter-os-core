// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

// The counter behind `preemption_disabled()` lives where the current execution
// context lives. In the guest that is one global: `main.rs` runs one kernel
// stack on one CPU, so "this context's depth" and "the machine's depth" are
// the same number, and the scheduler gates (task::scheduler tick and
// yield_now) read exactly what the lock holder raised.
//
// The host `--lib` binary has no such coincidence: libtest runs cases on many
// threads and every kernel `Mutex::lock` in any case bumps the same global, so
// "is preemption disabled for me" became "does anyone in the process hold any
// lock". That made the `sync::` assertions on `!preemption_disabled()` a race
// against unrelated neighbours' critical sections, which is why
// tests/test_runner_integrity.py's parallel sweep had to skip `sync::`. On the
// host the faithful model of one kernel stack per CPU is one counter per
// thread, so the test build gets a thread-local depth and the guest keeps the
// global verbatim — the `cfg(test)` path is compiled out of the bare-metal
// binary entirely. `std` is linked only for tests (see lib.rs), so this costs
// the guest nothing: no TLS machinery, no extra load, same `Relaxed` atomics.
#[cfg(not(test))]
mod preempt_depth {
    use core::sync::atomic::{AtomicU32, Ordering};

    static COUNT: AtomicU32 = AtomicU32::new(0);

    pub fn acquire() {
        COUNT.fetch_add(1, Ordering::Relaxed);
    }

    pub fn release() {
        COUNT.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn held() -> bool {
        COUNT.load(Ordering::Relaxed) != 0
    }
}

#[cfg(test)]
mod preempt_depth {
    use std::cell::Cell;
    use std::thread_local;

    // `const` init: no lazy-init allocation, and `Cell<u32>` has no destructor,
    // so thread exit cannot re-spawn TLS state mid-run.
    thread_local! {
        static COUNT: Cell<u32> = const { Cell::new(0) };
    }

    pub fn acquire() {
        COUNT.with(|c| c.set(c.get() + 1));
    }

    pub fn release() {
        COUNT.with(|c| c.set(c.get() - 1));
    }

    pub fn held() -> bool {
        COUNT.with(|c| c.get()) != 0
    }
}

/// Defers scheduler preemption until the returned guard is dropped.
#[derive(Debug)]
pub struct PreemptGuard(());

impl PreemptGuard {
    pub fn new() -> Self {
        preempt_depth::acquire();
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
        preempt_depth::release();
    }
}

/// Whether the current task holds a lock whose critical section must not be
/// interrupted by a context switch.
#[inline]
pub fn preemption_disabled() -> bool {
    preempt_depth::held()
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

    /// Deterministic pin for the host threading model: a neighbour thread
    /// holding a kernel lock must not disable preemption for a thread that
    /// holds none, and must not stop it from observing its own depth as 0
    /// once its own guards are gone. On the old process-global counter this
    /// case fails every time (the holder's +1 is visible everywhere); on the
    /// guest the two threads are the same kernel stack and the assertion
    /// pair collapses to the single-CPU semantics above.
    #[test]
    fn preempt_state_is_scoped_to_the_lock_holder_not_the_process() {
        use std::sync::mpsc;

        let m = Mutex::new(0u32);
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let holder = std::thread::spawn(move || {
            let g = m.lock();
            assert!(preemption_disabled(), "the holder must see its own guard");
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert!(preemption_disabled(), "the guard must stay live until release");
            drop(g);
            assert!(!preemption_disabled(), "dropping the last guard restores depth 0");
        });

        held_rx.recv().unwrap();
        assert!(
            !preemption_disabled(),
            "a thread holding no lock must not inherit another thread's deferred preemption"
        );
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(!preemption_disabled(), "no guard survives the join");
    }
}
