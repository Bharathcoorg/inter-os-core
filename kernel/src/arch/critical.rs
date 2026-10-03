//! Interrupt-masking shim for the bare-metal kernel and host unit tests.
//!
//! Every call site in this crate that took the `FRAMEBUFFER_WRITER` or
//! `SERIAL1` lock did so inside
//! `x86_64::instructions::interrupts::without_interrupts`, to stop an interrupt
//! handler deadlocking against the same spinlock.

/// Runs `f` with interrupts masked.
///
/// WHY: `without_interrupts` executes `cli`/`sti`, which are *privileged*
/// instructions. A host `cargo test` binary runs in ring 3, so any code path
/// reaching one died with `STATUS_PRIVILEGED_INSTRUCTION` (0xC0000096) before
/// its assertions could run — which is why tests such as
/// `drivers::display::tests::kms_adapter_enforces_surface_owner` appeared to
/// "fail" for reasons that had nothing to do with the code under test.
///
/// Interrupt masking has no meaning outside the bare-metal kernel: there is no
/// timer IRQ to exclude and no `cli` we are permitted to execute. So under
/// `cfg(test)` this simply calls `f`. The logic inside `f` — the part actually
/// under test — is unchanged and still runs in full.
#[cfg(not(test))]
#[inline]
pub fn with_interrupts_disabled<T>(f: impl FnOnce() -> T) -> T {
    x86_64::instructions::interrupts::without_interrupts(f)
}

/// Host test build: see the non-test variant for why masking is skipped.
#[cfg(test)]
#[inline]
pub fn with_interrupts_disabled<T>(f: impl FnOnce() -> T) -> T {
    f()
}
