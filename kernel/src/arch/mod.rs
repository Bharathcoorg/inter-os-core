// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Multi-Architecture Kernel Foundation
//!
//! Provides low-level boot-time abstractions: self-relocation for a higher-half
//! load and fastboot Android boot image verification.
//!
//! AArch64 drivers live in the kernel-aarch64 crate, not here. They cannot run
//! on this target, they duplicated that crate's own copies, and carrying them made
//! the x86_64 kernel run AArch64 hardware self-tests on an x86_64 machine.

pub mod critical;
pub mod fastboot;
pub mod relocate;

pub use critical::with_interrupts_disabled;
pub use fastboot::{FastbootBootImage, FastbootError, test_fastboot_boot_image};
pub use relocate::apply_relocations;

/// Initializes multi-architecture subsystems and tests.
pub fn init() {
    crate::serial_println!("--- [Multi-Architecture Hardware & Boot Image Layer] ---");

    // Initialize FPU/SSE hardware state in control registers CR0/CR4
    crate::task::context::init_fpu();

    // 1. Fastboot Android Boot Image Verification
    test_fastboot_boot_image();
}
