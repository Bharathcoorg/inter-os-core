// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Typed, bounds-checked Memory-Mapped I/O (MMIO) register abstraction.
//!
//! Driver code historically reached into physical address space with a bare
//! `base: usize` and an ad-hoc `unsafe fn` per peripheral. That form carries no
//! proof that the base is mapped, that a register offset stays inside the
//! peripheral window, that the target address is aligned for the access width,
//! or that the handle does not outlive the mapping.
//!
//! This module makes those obligations part of construction and the type system:
//!
//! - [`MmioBlock::from_mapped`] is `unsafe`: the caller must guarantee the base
//!   addresses a real, accessible MMIO window of `len` bytes that is owned for
//!   the returned block's lifetime `'a`. Range/alignment checks alone cannot
//!   prove an address is mapped, so safe code cannot mint a block from an
//!   arbitrary integer.
//! - A block carries the mapping lifetime `'a`, and [`MmioBlock::reg`] borrows
//!   the block, so a register handle cannot outlive the block or the mapping.
//! - The register value type `T` is restricted by the sealed [`RegisterWidth`]
//!   trait to `u8`/`u16`/`u32`/`u64` -- fixed-width integers in which every bit
//!   pattern is valid. An unrestricted `T: Copy` (which could be a `bool`, a
//!   `char`, or an enum with invalid bit patterns) is rejected at compile time.
//! - Direction is part of the type: [`Ro`]/[`Wo`]/[`Rw`] markers gate
//!   [`MmioReg::read`] and [`MmioReg::write`] through the [`CanRead`]/
//!   [`CanWrite`] traits, so a write-only register cannot be read and a
//!   read-only register cannot be written. There is deliberately no generic
//!   `modify`: read-modify-write is not atomic and is wrong for W1C/W0C/
//!   read-to-clear/write-only registers, so callers perform an explicit,
//!   documented read+write when the register semantics allow it.
//! - A block is `Send` but `!Sync` (it holds a `Cell`), so `&MmioBlock` is not
//!   `Send`: exclusive access is required to carve registers, matching the
//!   single-owner device model.
//!
//! Ordering: the `compiler_fence(SeqCst)` in [`MmioReg::read`]/[`MmioReg::write`]
//! orders the volatile accesses with respect to the compiler only. It is NOT a
//! substitute for the architecture-specific device ordering (e.g. `dmb`/`dsb`
//! on ARM64, `fence` on RISC-V) or for DMA cache maintenance; a driver that
//! needs those must issue them explicitly. Memory attributes (device memory,
//! non-cacheable) are the mapping's responsibility, established by the kernel
//! memory manager before `from_mapped` is called.
//!
//! The module is `no_std`, allocation-free, and host-unit-testable against a
//! fake `&'static mut [u8]` bus (see `tests`); it is not wired into any physical
//! driver yet.

use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::{align_of, size_of};
use core::num::NonZeroUsize;
use core::sync::atomic::{compiler_fence, Ordering};

mod private {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
}

/// Fixed-width integer types valid for a raw device register.
///
/// Sealed to `u8`/`u16`/`u32`/`u64`: every bit pattern of these is a valid
/// value, so a volatile read of any alignment-correct address is defined.
pub trait RegisterWidth: Copy + private::Sealed {}
impl RegisterWidth for u8 {}
impl RegisterWidth for u16 {}
impl RegisterWidth for u32 {}
impl RegisterWidth for u64 {}

/// Marker: the register may be read.
pub struct Ro;
/// Marker: the register may be written.
pub struct Wo;
/// Marker: the register may be read and written.
pub struct Rw;

/// Registers that support a read access.
pub trait CanRead {}
impl CanRead for Ro {}
impl CanRead for Rw {}

/// Registers that support a write access.
pub trait CanWrite {}
impl CanWrite for Wo {}
impl CanWrite for Rw {}

/// A validated, non-empty MMIO window `[base, base + len)` owned for `'a`.
///
/// Construct with the `unsafe` [`MmioBlock::from_mapped`], or (in tests) the
/// safe [`MmioBlock::from_fake_bus`]. A block is `Send` but `!Sync`, so carving
/// registers requires exclusive access.
pub struct MmioBlock<'a> {
    base: NonZeroUsize,
    len: usize,
    // `Cell` makes the block `!Sync` (so `&MmioBlock` is `!Send`), enforcing
    // exclusive access; the lifetime ties the block (and its registers) to the
    // mapping.
    _owner: PhantomData<Cell<&'a mut ()>>,
}

impl<'a> MmioBlock<'a> {
    /// Builds a block from a physical base address and byte length.
    ///
    /// # Safety
    /// `base` must address a real, accessible MMIO window of exactly `len` bytes
    /// that is mapped with device memory attributes and is owned (exclusively)
    /// for the returned block's lifetime `'a`. The caller must not unmap or
    /// reuse the window while the block or any register carved from it is live.
    /// Range and alignment are checked here, but mapping validity, memory
    /// attributes and lifetime are the caller's obligation -- they cannot be
    /// proven from integers alone.
    ///
    /// Returns `None` when `base` is zero, `len` is zero, or `base + len`
    /// overflows `usize`.
    pub unsafe fn from_mapped(base: usize, len: usize) -> Option<Self> {
        if base == 0 || len == 0 {
            return None;
        }
        base.checked_add(len)?;
        Some(Self {
            base: NonZeroUsize::new(base)?,
            len,
            _owner: PhantomData,
        })
    }

    /// Builds a block backed by a fake bus for host tests.
    ///
    /// The slice is borrowed for `'a`, so the block (and its registers) cannot
    /// outlive the backing storage. The base is the slice's own address; the
    /// test helper uses an over-aligned backing type so wider registers stay
    /// aligned.
    #[cfg(test)]
    pub fn from_fake_bus(bus: &'a mut [u8]) -> Self {
        let base = bus.as_ptr() as usize;
        let len = bus.len();
        Self {
            base: NonZeroUsize::new(base).expect("fake bus must be non-empty and non-null"),
            len,
            _owner: PhantomData,
        }
    }

    /// Carves a typed register at `offset` inside this block.
    ///
    /// Returns `None` unless `offset + size_of::<T>()` is within the block and
    /// the target address is aligned for `T`. `T` is restricted to
    /// [`RegisterWidth`] (`u8`/`u16`/`u32`/`u64`).
    pub fn reg<T: RegisterWidth, R>(&self, offset: usize) -> Option<MmioReg<'a, '_, T, R>> {
        let size = size_of::<T>();
        let align = align_of::<T>();
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
/// `T` is the access width ([`RegisterWidth`]); `R` is a direction marker
/// (`Ro`/`Wo`/`Rw`) that gates [`read`](MmioReg::read) and
/// [`write`](MmioReg::write). The handle borrows the block, so it cannot outlive
/// the block or the mapping.
pub struct MmioReg<'m, 'b, T: RegisterWidth, R> {
    block: &'b MmioBlock<'m>,
    offset: usize,
    _t: PhantomData<(T, R)>,
}

impl<'m, 'b, T: RegisterWidth, R> MmioReg<'m, 'b, T, R> {
    /// Raw address of the register. In-bounds and aligned by construction.
    #[inline]
    fn ptr(&self) -> *mut T {
        (self.block.base.get() + self.offset) as *mut T
    }

    /// Performs a volatile read of the register.
    ///
    /// # Safety
    /// `self` must originate from [`MmioBlock::reg`] (proving offset-in-bounds
    /// and alignment for `T`) and the block from the `unsafe`
    /// [`MmioBlock::from_mapped`] (proving the window is mapped, with device
    /// memory attributes, for the block's lifetime).
    #[inline]
    unsafe fn volatile_read(&self) -> T {
        core::ptr::read_volatile(self.ptr())
    }

    /// Performs a volatile write of the register.
    ///
    /// # Safety
    /// Same contract as [`volatile_read`](MmioReg::volatile_read).
    #[inline]
    unsafe fn volatile_write(&self, value: T) {
        core::ptr::write_volatile(self.ptr(), value)
    }

    /// Reads the register.
    ///
    /// Only available on readable registers (`R: CanRead`). The `SeqCst`
    /// compiler fences order the access for the compiler; they are NOT a device
    /// barrier (see the module docs on architecture-specific ordering).
    #[inline]
    pub fn read(&self) -> T
    where
        R: CanRead,
    {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: construction guarantees the pointer is in-bounds, aligned for
        // `T`, and the backing window is mapped for the block's lifetime.
        let value = unsafe { self.volatile_read() };
        compiler_fence(Ordering::SeqCst);
        value
    }

    /// Writes the register.
    ///
    /// Only available on writable registers (`R: CanWrite`).
    #[inline]
    pub fn write(&self, value: T)
    where
        R: CanWrite,
    {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: construction guarantees the pointer is in-bounds, aligned for
        // `T`, and the backing window is mapped for the block's lifetime.
        unsafe { self.volatile_write(value) };
        compiler_fence(Ordering::SeqCst);
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
        let reg: MmioReg<u32, Rw> = block.reg(0).expect("aligned u32 at offset 0");
        reg.write(0xDEAD_BEEF);
        assert_eq!(reg.read(), 0xDEAD_BEEF);
    }

    #[test]
    fn distinct_offsets_do_not_alias() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        let a: MmioReg<u32, Rw> = block.reg(0).unwrap();
        let b: MmioReg<u32, Rw> = block.reg(8).unwrap();
        a.write(1);
        b.write(2);
        assert_eq!(a.read(), 1);
        assert_eq!(b.read(), 2);
    }

    #[test]
    fn zero_base_is_rejected() {
        // SAFETY: base 0 is invalid; from_mapped must reject it before any use.
        assert!(unsafe { MmioBlock::from_mapped(0, 0x1000) }.is_none());
    }

    #[test]
    fn zero_len_is_rejected() {
        assert!(unsafe { MmioBlock::from_mapped(0x9000_0000, 0) }.is_none());
    }

    #[test]
    fn base_plus_len_overflow_is_rejected() {
        assert!(unsafe { MmioBlock::from_mapped(usize::MAX, 2) }.is_none());
    }

    #[test]
    fn out_of_range_offset_is_rejected() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        let reg: Option<MmioReg<u8, Rw>> = block.reg(64);
        assert!(reg.is_none());
    }

    #[test]
    fn misaligned_offset_is_rejected() {
        let block = MmioBlock::from_fake_bus(fake_bus());
        let reg: Option<MmioReg<u32, Rw>> = block.reg(2);
        assert!(reg.is_none());
    }

    #[test]
    fn write_only_register_cannot_be_read() {
        // Compile-time: `read` requires `R: CanRead`, which `Wo` does not impl.
        // This test only needs to construct the handle; calling read would not
        // compile. We assert construction and write work.
        let block = MmioBlock::from_fake_bus(fake_bus());
        let reg: MmioReg<u32, Wo> = block.reg(0).unwrap();
        reg.write(7);
    }
}