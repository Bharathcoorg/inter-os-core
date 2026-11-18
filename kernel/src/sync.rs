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
//!
//! # Dying inside a critical section
//!
//! Deferring preemption is not the same as surviving termination.
//! `task::scheduler::terminate_current` switches away from the exiting task
//! permanently: there is no unwind, so every guard on that task's kernel stack
//! is abandoned exactly where it stood. Before this module tracked them, an
//! abandoned guard left the preemption counter raised for the rest of the
//! device's life — the timer refused to preempt, `yield_now` refused to switch,
//! and the abandoned mutex made every later locker spin forever with
//! interrupts enabled. A user page fault taken inside a `Mutex` region (which
//! is what a kernel-mode copy from a user buffer can produce) was therefore a
//! whole-device hang rather than one dead task.
//!
//! Two pieces of state make that recoverable, both owned by the *execution
//! context* rather than by the lock:
//!
//! * every successful acquisition threads its mutex onto the context's
//!   held-lock list, and every guard drop unthreads it;
//! * [`abandon_current_context`] walks that list, force-unlocks each mutex and
//!   zeroes the context's preemption and interrupt depth.
//!
//! The fault handlers call it immediately before `terminate_current`, which is
//! the only point where the dying context's stack is still intact and the
//! scheduler has not yet been asked to lock anything.
//!
//! WHY force-unlocking every lock on the list is sound on this kernel: the
//! preemption gate means a task holding a `sync::Mutex` can never be switched
//! out, so at any instant every live guard belongs to the context that is
//! running. When that context dies, the list is exactly the set of locks nobody
//! will ever release again. Raw `spin::Mutex`es are not on the list and are not
//! touched; a subsystem that holds one across a fault has to recover it itself
//! (see `syscall::user_ptr::recover_abandoned_user_access`).

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

// The state behind `preemption_disabled()`, `in_interrupt_context()` and the
// held-lock list lives where the current execution context lives. In the guest
// that is one global: `main.rs` runs one kernel stack on one CPU, so "this
// context's depth" and "the machine's depth" are the same number, and the
// scheduler gates (task::scheduler tick and yield_now) read exactly what the
// lock holder raised.
//
// The host `--lib` binary has no such coincidence: libtest runs cases on many
// threads and every kernel `Mutex::lock` in any case bumps the same global, so
// "is preemption disabled for me" became "does anyone in the process hold any
// lock". That made the `sync::` assertions on `!preemption_disabled()` a race
// against unrelated neighbours' critical sections, which is why
// tests/test_runner_integrity.py's parallel sweep had to skip `sync::`. On the
// host the faithful model of one kernel stack per CPU is one counter per
// thread, so the test build gets thread-local state and the guest keeps the
// global verbatim — the `cfg(test)` path is compiled out of the bare-metal
// binary entirely. `std` is linked only for tests (see lib.rs), so this costs
// the guest nothing: no TLS machinery, no extra load, same `Relaxed` atomics.
//
// The held-lock list is per-context for the same reason and one more: a host
// neighbour thread's mutex must never be force-unlocked by this thread's
// recovery, so the list a recovery walks has to be the list this context built.
#[cfg(not(test))]
mod context_state {
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    static PREEMPT: AtomicU32 = AtomicU32::new(0);
    static IRQ: AtomicU32 = AtomicU32::new(0);
    static LOCKS: AtomicUsize = AtomicUsize::new(0);

    /// Decrements a depth counter, stopping at zero instead of wrapping.
    ///
    /// WHY not `fetch_sub`: `abandon_current_context` zeroes the depth for a
    /// context that is dying. If any guard of that context ever does run its
    /// `Drop` afterwards, a wrapping subtract leaves the count at `u32::MAX` --
    /// preemption frozen, or every keypress deferred forever, for the rest of
    /// the device's life. A release at zero must be a no-op.
    ///
    /// A CAS loop rather than `fetch_update`/`try_update`: the latter is renamed
    /// between toolchains and the kernel builds on a pinned nightly, so the
    /// stable primitive is the one that keeps compiling.
    fn saturating_dec(counter: &AtomicU32) {
        let mut depth = counter.load(Ordering::Relaxed);
        while depth != 0 {
            match counter.compare_exchange_weak(
                depth,
                depth - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(current) => depth = current,
            }
        }
    }

    pub fn preempt_acquire() {
        PREEMPT.fetch_add(1, Ordering::Relaxed);
    }

    pub fn preempt_release() {
        saturating_dec(&PREEMPT);
    }

    pub fn preempt_held() -> bool {
        PREEMPT.load(Ordering::Relaxed) != 0
    }

    pub fn irq_enter() {
        IRQ.fetch_add(1, Ordering::Relaxed);
    }

    pub fn irq_exit() {
        saturating_dec(&IRQ);
    }

    pub fn irq_held() -> bool {
        IRQ.load(Ordering::Relaxed) != 0
    }

    pub fn locks_head() -> usize {
        LOCKS.load(Ordering::Relaxed)
    }

    pub fn set_locks_head(addr: usize) {
        LOCKS.store(addr, Ordering::Relaxed);
    }

    /// Zeroes every depth this context raised. Only called when the context is
    /// being abandoned, so no guard of its will ever run its `Drop`.
    pub fn reset() {
        PREEMPT.store(0, Ordering::Relaxed);
        IRQ.store(0, Ordering::Relaxed);
        LOCKS.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod context_state {
    use std::cell::Cell;
    use std::thread_local;

    // `const` init: no lazy-init allocation, and `Cell` has no destructor, so
    // thread exit cannot re-spawn TLS state mid-run.
    thread_local! {
        static PREEMPT: Cell<u32> = const { Cell::new(0) };
        static IRQ: Cell<u32> = const { Cell::new(0) };
        static LOCKS: Cell<usize> = const { Cell::new(0) };
    }

    pub fn preempt_acquire() {
        PREEMPT.with(|c| c.set(c.get() + 1));
    }

    pub fn preempt_release() {
        // Saturating: see the guest version. A guard dropping after its
        // context was abandoned must not underflow the depth.
        PREEMPT.with(|c| c.set(c.get().saturating_sub(1)));
    }

    pub fn preempt_held() -> bool {
        PREEMPT.with(|c| c.get()) != 0
    }

    pub fn irq_enter() {
        IRQ.with(|c| c.set(c.get() + 1));
    }

    pub fn irq_exit() {
        IRQ.with(|c| c.set(c.get().saturating_sub(1)));
    }

    pub fn irq_held() -> bool {
        IRQ.with(|c| c.get()) != 0
    }

    pub fn locks_head() -> usize {
        LOCKS.with(|c| c.get())
    }

    pub fn set_locks_head(addr: usize) {
        LOCKS.with(|c| c.set(addr));
    }

    pub fn reset() {
        PREEMPT.with(|c| c.set(0));
        IRQ.with(|c| c.set(0));
        LOCKS.with(|c| c.set(0));
    }
}

/// Defers scheduler preemption until the returned guard is dropped.
#[derive(Debug)]
pub struct PreemptGuard(());

impl PreemptGuard {
    pub fn new() -> Self {
        context_state::preempt_acquire();
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
        context_state::preempt_release();
    }
}

/// Whether the current task holds a lock whose critical section must not be
/// interrupted by a context switch.
#[inline]
pub fn preemption_disabled() -> bool {
    context_state::preempt_held()
}

/// Marks the running context as an interrupt handler for as long as the guard
/// lives.
///
/// WHY the input pipeline needs this: `drivers::input::dispatch_to_focused_window`
/// is reached both from the keyboard IRQ and from normal task context, and the
/// two are not allowed to do the same work. In task context it may take the
/// compositor lock and flip a frame; in interrupt context it must only queue the
/// event, because the interrupted task may already hold that very lock and this
/// CPU has no second core to release it. The handler cannot tell the difference
/// from its own call stack, so the context says so explicitly.
#[derive(Debug)]
pub struct InterruptGuard(());

impl InterruptGuard {
    pub fn enter() -> Self {
        context_state::irq_enter();
        Self(())
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        context_state::irq_exit();
    }
}

/// Whether the current context is inside a hardware interrupt handler.
#[inline]
pub fn in_interrupt_context() -> bool {
    context_state::irq_held()
}

/// Type-erased link threading one held mutex onto its context's list.
///
/// `release` is fixed at construction; `owner` and `next` are written by the
/// context that acquires the mutex, which is the only context that can hold it.
#[derive(Debug)]
struct HeldLock {
    next: AtomicUsize,
    owner: AtomicUsize,
    release: unsafe fn(usize),
}

/// Force-unlocks `owner` if it is still locked.
///
/// # Safety
/// `owner` must be the address of a live `Mutex<T>`. Called only from
/// [`abandon_current_context`], whose invariant (see the module docs) is that
/// every lock on the list belongs to the context that is dying.
unsafe fn release_typed<T>(owner: usize) {
    let mutex = unsafe { &*(owner as *const Mutex<T>) };
    if mutex.inner.is_locked() {
        unsafe { mutex.inner.force_unlock() };
    }
}

/// Threads `node` onto the current context's held-lock list.
fn track(node: &HeldLock, owner: usize) -> usize {
    let addr = node as *const HeldLock as usize;
    node.owner.store(owner, Ordering::Relaxed);
    node.next.store(context_state::locks_head(), Ordering::Relaxed);
    context_state::set_locks_head(addr);
    addr
}

/// Unthreads the node at `addr`, wherever it sits in the list.
///
/// Guards do not have to drop in acquisition order — `drop(a)` before `b`, or
/// two guards held in one struct — so this cannot assume the node is the head.
fn untrack(addr: usize) {
    let mut prev = 0usize;
    let mut cur = context_state::locks_head();
    while cur != 0 {
        let node = unsafe { &*(cur as *const HeldLock) };
        let next = node.next.load(Ordering::Relaxed);
        if cur == addr {
            if prev == 0 {
                context_state::set_locks_head(next);
            } else {
                let prev_node = unsafe { &*(prev as *const HeldLock) };
                prev_node.next.store(next, Ordering::Relaxed);
            }
            return;
        }
        prev = cur;
        cur = next;
    }
}

/// Releases everything the dying context held and restores its depth counters.
///
/// Must be called *before* `task::scheduler::terminate_current`, on the context
/// that is about to be switched away from permanently. After that call the
/// guards on its stack will never drop, so this is the last chance to release
/// them. It takes no lock itself: it walks a list of addresses and pokes each
/// mutex's own atomic, which is safe to do from a fault handler.
///
/// Calling it on a context that holds nothing is a no-op, so the callers do not
/// have to know whether the fault landed inside a critical section.
pub fn abandon_current_context() {
    let mut cur = context_state::locks_head();
    // Detach first: a recovery must not be re-entered with a half-walked list.
    context_state::set_locks_head(0);
    while cur != 0 {
        let node = unsafe { &*(cur as *const HeldLock) };
        let next = node.next.load(Ordering::Relaxed);
        let owner = node.owner.load(Ordering::Relaxed);
        if owner != 0 {
            unsafe { (node.release)(owner) };
        }
        cur = next;
    }
    context_state::reset();
}

/// Spin mutex whose critical section is protected from scheduler preemption.
#[derive(Debug)]
pub struct Mutex<T: ?Sized> {
    held: HeldLock,
    inner: spin::Mutex<T>,
}

/// Guard returned by [`Mutex::lock`] and [`Mutex::try_lock`].
#[derive(Debug)]
pub struct MutexGuard<'a, T: ?Sized> {
    inner: spin::MutexGuard<'a, T>,
    /// Address of this guard's [`HeldLock`] link, for out-of-order removal.
    node: usize,
    // Declared last so the lock is released before preemption is restored.
    _preempt: PreemptGuard,
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        // Unthread before the lock goes free, so a recovery running on this
        // context can never see a node for a mutex it no longer holds.
        untrack(self.node);
    }
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            held: HeldLock {
                next: AtomicUsize::new(0),
                owner: AtomicUsize::new(0),
                release: release_typed::<T>,
            },
            inner: spin::Mutex::new(value),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        let preempt = PreemptGuard::new();
        let inner = self.inner.lock();
        // WHY the link is threaded after the acquisition and not before: only
        // the winner may push this node, otherwise two contenders would splice
        // the same link into the list twice. The few instructions between the
        // CAS and the push touch no user memory and call nothing, so a fault
        // there is a genuine kernel bug, not an abandoned user-buffer access.
        let node = track(&self.held, self as *const Self as usize);
        MutexGuard { inner, node, _preempt: preempt }
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let preempt = PreemptGuard::new();
        match self.inner.try_lock() {
            Some(inner) => {
                let node = track(&self.held, self as *const Self as usize);
                Some(MutexGuard { inner, node, _preempt: preempt })
            }
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

    // -------------------------------------------------------------------------
    // H9: a context that dies inside a critical section
    // -------------------------------------------------------------------------

    /// The defect, reproduced without a race.
    ///
    /// `terminate_current` switches away permanently and never unwinds, so the
    /// guards on the dead task's stack never run `Drop`. `mem::forget` is the
    /// exact host model of that: the guard is gone, its `Drop` will never run,
    /// and the lock and the preemption counter stay as the dead context left
    /// them. Before recovery existed this state was permanent — the timer
    /// refused to preempt, `yield_now` refused to switch, and every later
    /// locker of `m` span forever.
    #[test]
    fn a_context_that_dies_holding_locks_is_recoverable() {
        let outer = Mutex::new(1u32);
        let inner = Mutex::new(2u32);

        let g_outer = outer.lock();
        let g_inner = inner.lock();
        assert!(preemption_disabled());
        assert!(outer.try_lock().is_none());
        assert!(inner.try_lock().is_none());

        // The task dies here. No unwind: both guards are abandoned.
        core::mem::forget(g_outer);
        core::mem::forget(g_inner);

        // This is the leaked state the fault path used to leave behind.
        assert!(preemption_disabled(), "the abandoned guards still hold the gate up");
        assert!(outer.try_lock().is_none(), "the abandoned mutex is still held");
        assert!(inner.try_lock().is_none(), "the abandoned mutex is still held");

        abandon_current_context();

        assert!(!preemption_disabled(), "the gate must come back down with the task");
        assert!(!in_interrupt_context(), "interrupt depth must not survive the task either");
        {
            let mut a = outer.try_lock().expect("the abandoned outer mutex must be acquirable");
            let mut b = inner.try_lock().expect("the abandoned inner mutex must be acquirable");
            assert_eq!(*a, 1);
            assert_eq!(*b, 2);
            *a = 10;
            *b = 20;
        }
        assert!(!preemption_disabled());
        assert_eq!(*outer.lock(), 10);
        assert_eq!(*inner.lock(), 20);
    }

    /// Recovery must release the locks in whatever order the guards were
    /// abandoned, and must not leave a stale node behind that a later recovery
    /// would force-unlock a second time.
    #[test]
    fn recovery_survives_out_of_order_guard_drops_and_repeated_abandonment() {
        let a = Mutex::new(0u32);
        let b = Mutex::new(0u32);
        let c = Mutex::new(0u32);

        let ga = a.lock();
        let gb = b.lock();
        let gc = c.lock();
        // Drop the middle guard first: the list is not LIFO-ordered any more.
        drop(gb);
        assert!(b.try_lock().is_some(), "a dropped guard must unthread its node");
        core::mem::forget(ga);
        core::mem::forget(gc);

        abandon_current_context();
        assert!(a.try_lock().is_some());
        assert!(c.try_lock().is_some());
        assert!(!preemption_disabled());

        // A second recovery on a context that holds nothing is a no-op, and in
        // particular must not unlock anything a later acquisition took.
        let held = a.lock();
        abandon_current_context();
        drop(held);
        assert!(!preemption_disabled());
        assert!(a.try_lock().is_some());
    }

    /// A recovery on one context must not reach into another context's locks.
    /// On the guest there is only one context, so this is the host model of the
    /// same rule: the list a recovery walks is the list its own context built.
    #[test]
    fn recovery_does_not_unlock_a_neighbour_contexts_mutex() {
        use std::sync::mpsc;

        let shared = Mutex::new(0u32);
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let holder = std::thread::spawn(move || {
            let g = shared.lock();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            // Still held, and still this thread's own: the neighbour's
            // recovery must not have taken it away.
            assert!(shared.try_lock().is_none(), "a neighbour's recovery stole this lock");
            drop(g);
        });

        held_rx.recv().unwrap();
        let mine = Mutex::new(0u32);
        core::mem::forget(mine.lock());
        abandon_current_context();
        assert!(!preemption_disabled());
        release_tx.send(()).unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn interrupt_depth_is_scoped_to_the_handler_and_nests() {
        assert!(!in_interrupt_context());
        {
            let _outer = InterruptGuard::enter();
            assert!(in_interrupt_context());
            {
                let _inner = InterruptGuard::enter();
                assert!(in_interrupt_context());
            }
            assert!(in_interrupt_context(), "an exception inside a handler must not clear it");
        }
        assert!(!in_interrupt_context());
    }

    /// The keyboard handler's guard is abandoned with the rest of the context
    /// when a fault inside it terminates the task, so the depth must not stay
    /// raised and make every later input event defer itself forever.
    #[test]
    fn an_abandoned_interrupt_guard_does_not_freeze_input_deferral() {
        core::mem::forget(InterruptGuard::enter());
        assert!(in_interrupt_context());
        abandon_current_context();
        assert!(!in_interrupt_context());
    }

    /// A guard that drops *after* its context was abandoned must not underflow
    /// the depth. On the guest a wrapping subtract would leave the count at
    /// u32::MAX: preemption frozen and every keypress deferred forever, which is
    /// a worse failure than the leak the recovery exists to fix.
    #[test]
    fn a_guard_dropping_after_recovery_cannot_underflow_the_depth() {
        let lock = Mutex::new(0u32);
        let guard = lock.lock();
        let irq = InterruptGuard::enter();
        assert!(preemption_disabled());
        assert!(in_interrupt_context());

        abandon_current_context();
        assert!(!preemption_disabled());
        assert!(!in_interrupt_context());

        // The dying context's stack is gone on the guest, but a late Drop must
        // still be harmless rather than catastrophic.
        drop(guard);
        drop(irq);
        assert!(!preemption_disabled(), "a late release wrapped the preempt depth");
        assert!(!in_interrupt_context(), "a late release wrapped the interrupt depth");

        // And the counters still work normally afterwards.
        let again = lock.lock();
        assert!(preemption_disabled());
        drop(again);
        assert!(!preemption_disabled());
        let irq_again = InterruptGuard::enter();
        assert!(in_interrupt_context());
        drop(irq_again);
        assert!(!in_interrupt_context());
    }
}