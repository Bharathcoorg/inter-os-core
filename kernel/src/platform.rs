//! Truthful platform capability and verification contract.
//!
//! Gate D makes one rule explicit in the kernel: a feature may only be
//! presented as hardware-backed when the platform has an actual backend.
//! Emulators and unavailable devices remain explicit states.

#![allow(dead_code)]

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VerificationLevel {
    SourceOnly = 0,
    UnitTested = 1,
    QemuVerified = 2,
    HardwareVerified = 3,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HardwareAvailability {
    Unavailable = 0,
    Emulated = 1,
    Present = 2,
}

#[derive(Clone, Copy)]
pub struct PlatformContract {
    pub architecture: &'static str,
    pub verification: VerificationLevel,
    pub hardware: HardwareAvailability,
    pub persistent_state: bool,
    pub signed_packages: bool,
    pub isolated_userspace: bool,
}

impl PlatformContract {
    pub const fn qemu(architecture: &'static str) -> Self {
        Self {
            architecture,
            verification: VerificationLevel::QemuVerified,
            hardware: HardwareAvailability::Emulated,
            persistent_state: true,
            signed_packages: true,
            isolated_userspace: true,
        }
    }

    pub const fn is_hardware_backed(self) -> bool {
        matches!(self.hardware, HardwareAvailability::Present)
    }

    pub const fn is_verified(self) -> bool {
        !matches!(self.verification, VerificationLevel::SourceOnly)
    }

    pub const fn valid(self) -> bool {
        self.persistent_state && self.signed_packages && self.isolated_userspace
    }
}

/// Current x86_64 QEMU contract. Physical hardware is never inferred here.
pub static PLATFORM_CONTRACT: PlatformContract =
    PlatformContract::qemu("x86_64");

pub fn init() {
    debug_assert!(PLATFORM_CONTRACT.valid());
}
