// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS Kernel Library
//!
//! Everything the kernel is built from lives here so that the pure-logic modules
//! can be compiled and unit tested on the host. `boot.rs` is a thin binary that
//! wires the architecture subsystems together and enters the kernel; the
//! modules themselves must not depend on that binary, otherwise `cargo test`
//! cannot link them.

#![no_std]
#![cfg_attr(not(target_os = "windows"), feature(abi_x86_interrupt))]
// WHY gated exactly like the handler below: the `alloc_error_handler`
// attribute is still feature-gated on this nightly, and the gate must only
// be active for the bare-metal kernel build where the handler is defined.
#![cfg_attr(all(not(test), target_os = "none"), feature(alloc_error_handler))]

extern crate alloc;

// WHY: `cargo test` builds this library for the host, and the host test harness
// needs `std` for its allocator and test formatting. The bare-metal kernel build
// stays `no_std`; `std` is linked only into test builds.
#[cfg(test)]
extern crate std;

pub mod allocator;
pub mod arch;
pub mod audio;
pub mod capability;
pub mod compat;
pub mod compositor;
pub mod content;
pub mod crypto;
pub mod diagnostics;
pub mod display_hal;
pub mod drivers;
pub mod event;
pub mod framebuffer;
pub mod fs;
pub mod gdt;
pub mod gui;
pub mod hal;
pub mod input;
pub mod interrupts;
pub mod ipc;
pub mod memory;
pub mod net;
pub mod notification;
pub mod package;
pub mod performance;
pub mod panic;
pub mod platform;
pub mod recovery;
pub mod resource;
pub mod sensor;
pub mod serial;
pub mod service_abi;
pub mod services;
pub mod settings;
pub mod storage;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod telephony;
pub mod update;
pub mod user;
pub mod wasm;
pub mod watchdog;

// ---------------------------------------------------------------------------
// Kernel heap exhaustion policy (alloc_error_handler / `oom` lang item).
//
// WHY this lives in the library and not in main.rs: the lang item must exist
// in every bare-metal link of this crate, and the library is the shared unit
// the boot binary links against. main.rs owns the `#[panic_handler]`; this is
// the separate allocation-failure path the allocator invokes when the 64 MiB
// kernel heap cannot satisfy a request.
//
// WHY the cfg gate: on the host (`cargo test` / `cargo check`) `std` is linked
// and already provides the `oom` lang item; defining a second one is a
// duplicate-lang-item link error. The handler therefore exists only for the
// bare-metal kernel target, never for test builds.
//
// WHY halt instead of panic: an OOM can fire *inside* the panic handler or
// while formatting a panic message, so calling panic!() here risks
// panic-in-panic recursion. The policy is: disable interrupts, report the
// failed layout over the serial console, and halt. The serial path
// (`serial::_print_internal`) uses only bounded `try_lock()` attempts and
// performs no allocation, so it is safe here; if the UART lock is already
// held the message is dropped after the bounded retries and the CPU still
// halts. No blocking `.lock()` is taken anywhere in this path.
//
// Verification status: defined and compile-checked for x86_64-unknown-none;
// NOT device-exercised (no QEMU/hardware run was performed for this change).
// ---------------------------------------------------------------------------
#[cfg(all(not(test), target_os = "none", target_arch = "x86_64"))]
#[alloc_error_handler]
fn interos_alloc_error(layout: core::alloc::Layout) -> ! {
    x86_64::instructions::interrupts::disable();
    crate::serial_println!(
        "\n[OOM] kernel heap allocation failed: size={} align={}",
        layout.size(),
        layout.align()
    );
    crate::serial_println!("[OOM] policy: HALT (allocator cannot recover; no panic recursion)");
    loop {
        x86_64::instructions::hlt();
    }
}
