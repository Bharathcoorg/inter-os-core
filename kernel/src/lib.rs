//! Inter OS Kernel Library
//!
//! Everything the kernel is built from lives here so that the pure-logic modules
//! can be compiled and unit tested on the host. `boot.rs` is a thin binary that
//! wires the architecture subsystems together and enters the kernel; the
//! modules themselves must not depend on that binary, otherwise `cargo test`
//! cannot link them.

#![no_std]
#![cfg_attr(not(target_os = "windows"), feature(abi_x86_interrupt))]

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
pub mod syscall;
pub mod task;
pub mod telephony;
pub mod update;
pub mod user;
pub mod wasm;
pub mod watchdog;
