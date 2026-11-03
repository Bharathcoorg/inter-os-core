// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS architecture-neutral kernel core.
//!
//! The repository has three kernels that were three separate implementations:
//! x86_64 (141 modules), AArch64 (48 modules) and RISC-V (7 modules). Everything
//! that is not about the CPU belongs in here once, so that a fix to the journal,
//! the VFS or the keystore is one change instead of three.
//!
//! This crate is the first extraction, and it is deliberately the subsystem with
//! the fewest architecture assumptions: the crash-consistent metadata journal
//! (`storage::persistent`), the guard that refuses to overwrite the running
//! kernel image (`storage::boot_guard`), the platform-neutral block device
//! contract both of them speak (`storage::block`) and the two primitives the
//! journal's record tags are built from (`crypto::sha256`, `crypto::hmac`).
//!
//! Everything here is `no_std` and free of CPU-specific code, and it compiles for
//! `x86_64-unknown-none`, `aarch64-unknown-none` and `riscv64gc-unknown-none-elf`
//! as well as for the host, where its unit tests run.
//!
//! # What the architecture still owns
//!
//! The core owns logic and refuses to name hardware. Where a decision needs the
//! machine, the core declares the seam and the kernel supplies the part:
//!
//! | seam | declared in | supplied by |
//! |------|-------------|-------------|
//! | block device | [`storage::persistent::bind_block_device`] | the ATA driver on x86, SDHCI/eMMC or virtio-blk on AArch64, virtio-blk on RISC-V |
//! | serial console | [`console::install_console`] | 16550/COM1 on x86, PL011 on AArch64, SBI DBCN on RISC-V |
//! | kernel image extent | [`storage::boot_guard::install_image_extent_source`] | the ELF program headers on x86, the linker bounds on AArch64, TBD on RISC-V |
//!
//! Those three are the whole cost of moving the journal off x86. That is the
//! point: a subsystem whose entire architecture surface is three seams can be
//! extracted, and the next ones (VFS, InterFS, keystore, IPC) can follow the
//! same shape.

#![no_std]
// WHY: `cargo test` builds this library for the host, and the host test harness
// needs `std` for its allocator and test formatting. The bare-metal kernel build
// stays `no_std`; `std` is linked only into test builds.
#[cfg(test)]
extern crate std;

extern crate alloc;

pub mod console;
pub mod crypto;
pub mod storage;
