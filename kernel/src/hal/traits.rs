// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Mobile SoC Hardware Abstraction Layer (HAL) Traits
//!
//! Provides hardware-neutral interfaces for multi-architecture mobile operating systems.
//! Enforces an explicit distinction between QEMU/development virtual adapters
//! and bare-metal physical hardware drivers.

/// Target CPU architecture enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlatformArch {
    X86_64 = 0,
    AArch64 = 1,
    RiscV64 = 2,
}

/// Provenance and reality classification of a driver implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    /// Virtualized or emulated device provided by QEMU / hypervisor.
    VirtualQemu,
    /// Physical bare-metal hardware driver bound to real silicon registers.
    PhysicalHardware,
    /// Pure software simulator or test fixture used for deterministic testing.
    Simulator,
    /// Device class is defined by HAL contract but hardware is not present.
    NotPresent,
}

impl DriverKind {
    pub const fn is_physical(&self) -> bool {
        matches!(self, Self::PhysicalHardware)
    }

    pub const fn is_virtual(&self) -> bool {
        matches!(self, Self::VirtualQemu)
    }
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
    fn driver_kind(&self) -> DriverKind {
        DriverKind::VirtualQemu
    }
    /// Returns monotonic hardware clock ticks since system boot.
    fn read_ticks(&self) -> u64;
    /// Returns hardware counter nominal frequency in Hz.
    fn frequency_hz(&self) -> u64;
}

/// Platform serial communication console interface.
pub trait SerialConsole: Send + Sync {
    fn driver_kind(&self) -> DriverKind {
        DriverKind::VirtualQemu
    }
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
    fn driver_kind(&self) -> DriverKind {
        DriverKind::VirtualQemu
    }
    /// Enters low-power wait-for-interrupt CPU state (HLT on x86, WFI on ARM).
    fn sleep_cpu(&self);
    /// Requests hardware system reset.
    fn reset(&self) -> !;
    /// Requests hardware system power-off.
    fn shutdown(&self) -> !;
}

/// Display and scanout controller HAL.
pub trait DisplayHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn resolution(&self) -> (u32, u32);
    fn flush_scanout(&mut self) -> Result<(), &'static str>;
}

/// Touch digitizer and input controller HAL.
pub trait TouchInputHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn poll_touch(&mut self) -> Option<(u32, u32, bool)>;
}

/// Block storage device HAL.
pub trait StorageHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn sector_count(&self) -> u64;
    fn read_sector(&mut self, lba: u64, buf: &mut [u8; 512]) -> Result<(), &'static str>;
    fn write_sector(&mut self, lba: u64, buf: &[u8; 512]) -> Result<(), &'static str>;
    fn flush(&mut self) -> Result<(), &'static str>;
}

/// Network adapter interface HAL.
pub trait NetworkHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn mac_address(&self) -> [u8; 6];
    fn transmit(&mut self, packet: &[u8]) -> Result<(), &'static str>;
    fn receive(&mut self, buf: &mut [u8]) -> Result<usize, &'static str>;
}

/// Bluetooth controller HAL interface.
pub trait BluetoothHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn is_ready(&self) -> bool;
    fn send_hci_cmd(&mut self, opcode: u16, params: &[u8]) -> Result<(), &'static str>;
}

/// Audio output and codec HAL interface.
pub trait AudioHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn play_tone(&mut self, frequency_hz: u32, duration_ms: u32) -> Result<(), &'static str>;
    fn set_volume(&mut self, percent: u8);
}

/// Camera ISP and sensor HAL interface.
pub trait CameraHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn capture_frame(&mut self, out_buffer: &mut [u8]) -> Result<usize, &'static str>;
}

/// Sensor bus and IMU HAL interface.
pub trait SensorHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn sample_accelerometer(&mut self) -> Result<(i32, i32, i32), &'static str>;
    fn sample_ambient_lux(&mut self) -> Result<u32, &'static str>;
}

/// Baseband modem HAL interface.
pub trait ModemHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn send_at_command(&mut self, cmd: &str, resp_buf: &mut [u8]) -> Result<usize, &'static str>;
}

/// PMIC and battery telemetry HAL interface.
pub trait PmicHal: Send + Sync {
    fn driver_kind(&self) -> DriverKind;
    fn battery_percentage(&self) -> u8;
    fn temperature_millicelsius(&self) -> i32;
    fn is_charging(&self) -> bool;
}
