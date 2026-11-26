// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Typed, bounds-checked Memory-Mapped I/O (MMIO) register abstraction.
//!
//! Driver code historically reached into physical address space with a bare
//! `base: usize` and an ad-hoc `unsafe fn` per peripheral. That form carries no
//! proof that the base is mapped, that a register offset stays inside the
//! peripheral window, or that the target address is aligned for the access
//! width. Every one of those is a runtime fault or silent corruption waiting
//! for a wiring mistake.
//!
//! This module makes those obligations part of construction:
//!
//! - [`MmioBlock::from_mapped`] rejects a zero base, a zero length, and a
//!   `base + len` that overflows the address space, so a valid block always
//!   describes a non-empty, in-range window.
//! - [`MmioBlock::reg`] only yields a [`MmioReg`] when `offset + size_of::<T>()`
//!   stays inside the block *and* the resulting address is aligned for `T`.
//! - The safe [`MmioReg::read`] / [`MmioReg::write`] / [`MmioReg::modify`]
//!   methods are the only way to touch the register. The single `unsafe` is the
//!   internal volatile access, whose soundness is discharged by the checks
//!   above.
//!
//! The register type parameter `T` is the access width (`u8`/`u16`/`u32`/`u64`
//! or a `#[repr(C)]` register struct); `R` is a zero-sized marker that keeps
//! two registers of the same width at the same offset distinct at the type
//! level. The module is `no_std`, allocation-free, and host-unit-testable
//! against a fake `&'static mut [u8]` bus (see the `tests` module); it is not
//! wired into any physical driver yet.

use core::marker::PhantomData;
use core::mem::{align_of, size_of};
use core::num::NonZeroUsize;
use core::sync::atomic::{compiler_fence, Ordering};

/// A validated, non-empty MMIO window `[base, base + len)`.
///
/// Construct with [`MmioBlock::from_mapped`]. A block never wraps the address
/// space and never has a zero base or length, so any register carved from it is
/// in-bounds by construction.
pub struct MmioBlock {
    base: NonZeroUsize,
    len: usize,
}

impl MmioBlock {
    /// Builds a block from a physical base address and byte length.
    ///
    /// Returns `None` when `base` is zero, `len` is zero, or `base + len`
    /// overflows `usize`. Callers turn the `Option` into a hard error at the
    /// driver boundary rather than carrying an unvalidated address forward.
    pub fn from_mapped(base: usize, len: usize) -> Option<Self> {
        if base == 0 || len == 0 {
            return None;
        }
        // Reject a window whose end wraps past the top of the address space.
        base.checked_add(len)?;
        Some(Self {
            base: NonZeroUsize::new(base)?,
            len,
        })
    }

    /// Builds a block backed by a fake bus for host tests.
    ///
    /// The slice must be `'static` so the block's borrow is valid for the whole
    /// test; the base is the slice's own address. Alignment of the slice is the
    /// caller's responsibility (the test helper uses an over-aligned backing
    /// type so wider registers stay aligned).
    #[cfg(test)]
    pub fn from_fake_bus(bus: &'static mut [u8]) -> Self {
        let base = bus.as_ptr() as usize;
        let len = bus.len();
        Self {
            base: NonZeroUsize::new(base).expect("fake bus must be non-empty and non-null"),
            len,
        }
    }

    /// Carves a typed register at `offset` inside this block.
    ///
    /// Returns `None` unless `offset + size_of::<T>()` is within the block and
    /// the target address is aligned for `T`. A zero-sized `T` is rejected
    /// because it addresses no hardware.
    pub fn reg<T, R>(&self, offset: usize) -> Option<MmioReg<'_, T, R>>
    where
        T: Copy,
    {
        let size = size_of::<T>();
        let align = align_of::<T>();
        if size == 0 {
            return None;
        }
        let end = offset.checked_add(size)?;
        if end > self.len {
            return None;
        }
        // `base + offset` cannot overflow: `offset < end <= len` and
        // `base + len` was checked at construction.
        if (self.base.get() + offset) % align != 0 {
            return None;
        }
        Some(MmioReg {
            block: self,
            offset,
            _t: PhantomData,
        })
    }
}

/// A typed handle to one MMIO register carved from an [`MmioBlock`].
///
/// `T` is the access width, `R` is a zero-sized marker distinguishing
/// otherwise-identical registers. All access goes through the safe
/// [`read`](MmioReg::read) / [`write`](MmioReg::write) /
/// [`modify`](MmioReg::modify) methods.
pub struct MmioReg<'a, T: Copy, R> {
    block: &'a MmioBlock,
    offset: usize,
    _t: PhantomData<(T, R)>,
}

impl<'a, T: Copy, R> MmioReg<'a, T, R> {
    /// Raw address of the register. In-bounds and aligned by construction.
    #[inline]
    fn ptr(&self) -> *mut T {
        (self.block.base.get() + self.offset) as *mut T
    }

    /// Performs a volatile read of the register.
    ///
    /// # Safety
    /// `self` must originate from [`MmioBlock::reg`], which proves
    /// `offset + size_of::<T>() <= block.len` and that the address is aligned
    /// for `T`; [`MmioBlock::from_mapped`] proves the base is non-zero and the
    /// window does not overflow the address space. Together these make the
    /// pointer valid for a `T` access.
    #[inline]
    unsafe fn volatile_read(&self) -> T {
        core::ptr::read_volatile(self.ptr())
    }

    /// Performs a volatile write of the register.
    ///
    /// # Safety
    /// Same contract as [`volatile_read`](MmioReg::volatile_read): construction
    /// guarantees the pointer is in-bounds and aligned for `T`.
    #[inline]
    unsafe fn volatile_write(&self, value: T) {
        core::ptr::write_volatile(self.ptr(), value)
    }

    /// Reads the register with full compiler ordering.
    #[inline]
    pub fn read(&self) -> T {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: `self` was produced by `MmioBlock::reg`, which validated
        // offset-in-bounds and alignment for `T`.
        let value = unsafe { self.volatile_read() };
        compiler_fence(Ordering::SeqCst);
        value
    }

    /// Writes the register with full compiler ordering.
    #[inline]
    pub fn write(&self, value: T) {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: `self` was produced by `MmioBlock::reg`, which validated
        // offset-in-bounds and alignment for `T`.
        unsafe { self.volatile_write(value) };
        compiler_fence(Ordering::SeqCst);
    }

    /// Applies `f` to the current value and writes the result back.
    ///
    /// This is a read-modify-write, not an atomic one: it is intended for
    /// device registers, not shared memory. The surrounding `SeqCst` fences
    /// order the two accesses with respect to other volatile accesses.
    #[inline]
    pub fn modify<F>(&self, f: F)
    where
        F: FnOnce(T) -> T,
    {
        let old = self.read();
        let new = f(old);
        self.write(new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over-aligned backing store so wider registers stay aligned regardless of
    /// the allocator's base alignment for a plain `u8` allocation.
    #[repr(align(8))]
    struct AlignedBus([u8; 64]);

    fn fake_bus() -> &'static mut [u8] {
        let boxed = std::boxed::Box::new(AlignedBus([0u8; 64]));
        let leaked: &'static mut AlignedBus = std::boxed::Box::leak(boxed);
        &mut leaked.0
    }

    #[test]
    fn write_then_read_roundtrips_through_fake_bus() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        let reg: MmioReg<u32, ()> = block.reg(0).expect("aligned u32 at offset 0");
        reg.write(0xDEAD_BEEF);
        assert_eq!(reg.read(), 0xDEAD_BEEF);
    }

    #[test]
    fn modify_applies_closure_to_current_value() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        let reg: MmioReg<u32, ()> = block.reg(4).expect("aligned u32 at offset 4");
        reg.write(10);
        reg.modify(|v| v + 5);
        assert_eq!(reg.read(), 15);
    }

    #[test]
    fn distinct_offsets_do_not_alias() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        let a: MmioReg<u32, ()> = block.reg(0).unwrap();
        let b: MmioReg<u32, ()> = block.reg(8).unwrap();
        a.write(1);
        b.write(2);
        assert_eq!(a.read(), 1);
        assert_eq!(b.read(), 2);
    }

    #[test]
    fn zero_base_is_rejected() {
        assert!(MmioBlock::from_mapped(0, 0x1000).is_none());
    }

    #[test]
    fn zero_len_is_rejected() {
        assert!(MmioBlock::from_mapped(0x9000_0000, 0).is_none());
    }

    #[test]
    fn base_plus_len_overflow_is_rejected() {
        assert!(MmioBlock::from_mapped(usize::MAX, 2).is_none());
    }

    #[test]
    fn out_of_range_offset_is_rejected() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        // 64 + 1 > 64: one past the end of the 64-byte window.
        let reg: Option<MmioReg<u8, ()>> = block.reg(64);
        assert!(reg.is_none());
    }

    #[test]
    fn misaligned_offset_is_rejected() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        // Base is 8-aligned; offset 2 breaks 4-byte alignment for a u32.
        let reg: Option<MmioReg<u32, ()>> = block.reg(2);
        assert!(reg.is_none());
    }
}