//! Mobile SoC Hardware Abstraction Layer (HAL) Traits
//!
//! Provides hardware-neutral interfaces for multi-architecture mobile operating systems.
//! Shields the microkernel from target-specific differences between x86_64 QEMU/PC
//! and ARM64 mobile SoCs (Snapdragon, Dimensity, Tensor, Apple Silicon).

/// Target CPU architecture enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlatformArch {
    X86_64 = 0,
    AArch64 = 1,
    RiscV64 = 2,
}

/// Generic mobile SoC platform descriptor.
#[derive(Debug, Clone, Copy)]
pub struct PlatformInfo {
    pub arch: PlatformArch,
    pub soc_name: &'static str,
    pub board_name: &'static str,
    pub cpu_cores: usize,
    pub base_frequency_mhz: u32,
    pub total_ram_bytes: usize,
    pub dtb_base: Option<usize>,
}

/// Platform-level high-precision monotonic hardware timer.
pub trait PlatformTimer: Send + Sync {
    /// Returns monotonic hardware clock ticks since system boot.
    fn read_ticks(&self) -> u64;
    /// Returns hardware counter nominal frequency in Hz.
    fn frequency_hz(&self) -> u64;
}

/// Platform serial communication console interface.
pub trait SerialConsole: Send + Sync {
    /// Transmits a single byte over the hardware serial port.
    fn write_byte(&self, byte: u8);
    /// Transmits a UTF-8 string slice over the serial console.
    fn write_str(&self, s: &str) {
        for b in s.bytes() {
            self.write_byte(b);
        }
    }
    /// Attempts to read a received byte without blocking.
    fn read_byte(&self) -> Option<u8>;
}

/// Mobile SoC power state and system lifecycle controller.
pub trait PowerController: Send + Sync {
    /// Enters low-power wait-for-interrupt CPU state (HLT on x86, WFI on ARM).
    fn sleep_cpu(&self);
    /// Requests hardware system reset.
    fn reset(&self) -> !;
    /// Requests hardware system power-off.
    fn shutdown(&self) -> !;
}
