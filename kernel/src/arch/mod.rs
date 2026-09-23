//! Multi-Architecture Kernel Foundation
//!
//! Provides platform-specific low-level hardware abstractions, exception vector tables,
//! fastboot Android boot image verification, and AArch64 mobile SoC drivers.

pub mod aarch64;
pub mod fastboot;

pub use fastboot::{FastbootBootImage, FastbootError, test_fastboot_boot_image};

/// Initializes multi-architecture subsystems and tests.
pub fn init() {
    crate::serial_println!("--- [Multi-Architecture Hardware & Boot Image Layer] ---");

    // 1. Fastboot Android Boot Image Verification
    test_fastboot_boot_image();

    // 2. AArch64 Smartphone Subsystem Verification
    aarch64::init();
}
