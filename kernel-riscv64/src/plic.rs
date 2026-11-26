// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V Platform-Level Interrupt Controller (PLIC) Driver
//!
//! Targets the SiFive `sifive,plic-1.0` / `riscv,plic0` controller found on the
//! QEMU `virt` board (and SiFive/StarFive/T-Head SoCs). Implements the full
//! register surface required to route external interrupts to S-mode:
//! source priority, per-context enable bits, per-context threshold, and the
//! claim/complete handshake.
//!
//! The controller base address is taken from the parsed DTB (`fdt::board_info`)
//! and only falls back to the documented QEMU `virt` base when no DTB was
//! available. External interrupts (`scause` code 9) are claimed here; the
//! current handler records the claim and completes it (a documented stub — the
//! per-IRQ device handler registry is a remaining gap, see module notes).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// QEMU `virt` PLIC aperture (fallback when no DTB is present).
pub const DEFAULT_BASE: usize = 0x0c00_0000;

/// Per-source priority register stride (1024 sources x 4 bytes).
const PRIORITY_STRIDE: usize = 0x4;
/// Enable-bit block offset from base.
const ENABLE_BASE_OFFSET: usize = 0x2000;
/// Enable-bit block per context (32 words x 4 bytes = 0x80).
const ENABLE_STRIDE: usize = 0x80;
/// Claim/complete block offset from base.
const CONTEXT_CLAIM_OFFSET: usize = 0x20_0000;
/// Threshold block offset from base.
const CONTEXT_THRESHOLD_OFFSET: usize = 0x20_1000;

/// A PLIC controller bound to a physical base address.
#[derive(Copy, Clone, Debug)]
pub struct Plic {
    base: usize,
}

impl Plic {
    /// Documented QEMU `virt` base.
    pub const DEFAULT_BASE: usize = DEFAULT_BASE;

    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    fn priority_ptr(&self, irq: u32) -> *mut u32 {
        (self.base + irq as usize * PRIORITY_STRIDE) as *mut u32
    }

    fn enable_ptr(&self, context: usize, irq: u32) -> *mut u32 {
        (self.base + ENABLE_BASE_OFFSET + context * ENABLE_STRIDE + (irq as usize / 32) * 4)
            as *mut u32
    }

    fn claim_ptr(&self, context: usize) -> *mut u32 {
        (self.base + CONTEXT_CLAIM_OFFSET + context * 4) as *mut u32
    }

    fn complete_ptr(&self, context: usize) -> *mut u32 {
        (self.base + CONTEXT_CLAIM_OFFSET + context * 4 + 4) as *mut u32
    }

    fn threshold_ptr(&self, context: usize) -> *mut u32 {
        (self.base + CONTEXT_THRESHOLD_OFFSET + context * 4) as *mut u32
    }

    /// Sets the priority (0 = never interrupt, 1..7 = increasing urgency).
    ///
    /// # Safety
    /// `base` must map a live PLIC.
    pub unsafe fn set_priority(&self, irq: u32, priority: u32) {
        write_volatile(self.priority_ptr(irq), priority & 0x7);
    }

    /// Enables `irq` for `context`.
    ///
    /// # Safety
    /// `base` must map a live PLIC.
    pub unsafe fn enable(&self, context: usize, irq: u32) {
        let p = self.enable_ptr(context, irq);
        let v = read_volatile(p) | (1u32 << (irq % 32));
        write_volatile(p, v);
    }

    /// Disables `irq` for `context`.
    ///
    /// # Safety
    /// `base` must map a live PLIC.
    pub unsafe fn disable(&self, context: usize, irq: u32) {
        let p = self.enable_ptr(context, irq);
        let v = read_volatile(p) & !(1u32 << (irq % 32));
        write_volatile(p, v);
    }

    /// Sets the priority threshold for `context`; interrupts at or below the
    /// threshold are masked.
    ///
    /// # Safety
    /// `base` must map a live PLIC.
    pub unsafe fn set_threshold(&self, context: usize, threshold: u32) {
        write_volatile(self.threshold_ptr(context), threshold & 0x7);
    }

    /// Claims the highest-priority pending interrupt for `context`.
    /// Returns the source id, or 0 when nothing is pending (spurious).
    ///
    /// # Safety
    /// `base` must map a live PLIC.
    pub unsafe fn claim(&self, context: usize) -> u32 {
        read_volatile(self.claim_ptr(context))
    }

    /// Signals completion of `irq` for `context`, re-enabling lower-priority
    /// sources.
    ///
    /// # Safety
    /// `base` must map a live PLIC.
    pub unsafe fn complete(&self, context: usize, irq: u32) {
        write_volatile(self.complete_ptr(context), irq);
    }

    /// S-mode context id for a hart. QEMU `virt` assigns each hart an M-mode
    /// context (`2*hart`) followed by an S-mode context (`2*hart + 1`).
    pub fn supervisor_context(hart: usize) -> usize {
        hart * 2 + 1
    }
}

static PLIC_BASE: AtomicUsize = AtomicUsize::new(0);
static LAST_CLAIMED_IRQ: AtomicU32 = AtomicU32::new(0);
static CLAIM_COUNT: AtomicU64 = AtomicU64::new(0);

/// Binds the driver to the discovered (or default) PLIC base.
pub fn init(base: usize) {
    PLIC_BASE.store(base, Ordering::SeqCst);
}

/// Returns the active PLIC base (FDT-derived, else QEMU `virt` default).
pub fn base() -> usize {
    let b = PLIC_BASE.load(Ordering::SeqCst);
    if b == 0 {
        crate::fdt::board_info().plic_base as usize
    } else {
        b
    }
}

/// Services an external interrupt for `hart`: claims the top source, records
/// it, and completes it.
///
/// This is a documented stub: it proves the claim/complete handshake against
/// real controller state and records the claim, but dispatch to a per-device
/// handler table is a remaining gap (see `kernel-riscv64` bring-up notes).
///
/// # Safety
/// Must be called from the external-interrupt trap path with a live PLIC.
pub unsafe fn dispatch_external(hart: usize) {
    let plic = Plic::new(base());
    let context = Plic::supervisor_context(hart);
    let irq = plic.claim(context);
    LAST_CLAIMED_IRQ.store(irq, Ordering::SeqCst);
    if irq != 0 {
        CLAIM_COUNT.fetch_add(1, Ordering::SeqCst);
        // Documented stub: record the claim, then complete so the controller
        // does not re-present the same source indefinitely.
        plic.complete(context, irq);
    }
}

/// Last claimed external IRQ source id (0 if none/spurious).
pub fn last_claimed_irq() -> u32 {
    LAST_CLAIMED_IRQ.load(Ordering::SeqCst)
}

/// Total number of non-spurious external IRQs claimed since boot.
pub fn claim_count() -> u64 {
    CLAIM_COUNT.load(Ordering::SeqCst)
}