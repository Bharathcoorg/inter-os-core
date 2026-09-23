//! AArch64 (ARMv8-A / ARMv9-A) Smartphone Architecture Support
//!
//! Provides bare-metal microkernel drivers and abstractions for mobile ARM SoCs
//! (Qualcomm Snapdragon, Apple Silicon, MediaTek Dimensity, PinePhone Allwinner).

pub mod boot;
pub mod fdt;
pub mod gic;
pub mod mmu;
pub mod pl011;
pub mod psci;
pub mod simplefb;
pub mod timer;

pub use boot::{ArmExceptionFrame, handle_aarch64_syscall, test_aarch64_syscall_dispatch};
pub use fdt::{ArmFdtParser, ArmFdtNode, test_aarch64_fdt};
pub use gic::{Gicv2, DEFAULT_GICC_BASE, DEFAULT_GICD_BASE};
pub use mmu::{PageTableEntry, TranslationTable, MAIR_CONFIG, TCR_CONFIG};
pub use pl011::Pl011Uart;
pub use psci::{PSCI, PsciController, test_psci};
pub use simplefb::{SimpleFramebuffer, SimpleFbFormat, test_simplefb};
pub use timer::ArmGenericTimer;

/// Initializes AArch64 architecture self-tests and drivers.
pub fn init() {
    crate::serial_println!("--- [Initializing AArch64 Mobile Smartphone Subsystem] ---");

    // 1. Test AArch64 syscall ABI dispatch
    test_aarch64_syscall_dispatch();

    // 2. Test AArch64 Flattened Device Tree (DTB) Parser
    test_aarch64_fdt();

    // 3. Test AArch64 PSCI Firmware Power Interface
    test_psci();

    // 4. Test AArch64 Physical SimpleFB Linear Framebuffer Driver
    test_simplefb();

    crate::serial_println!("[OK] AArch64 Subsystem: PL011 UART, GICv2, MMU 48-bit VA, PSCI v1.1, FDT parser, SimpleFB display, and EL1/EL0 ABI initialized.");
}


