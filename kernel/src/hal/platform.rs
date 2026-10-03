//! Concrete Platform Implementations for x86_64 and AArch64 Mobile SoCs

use super::traits::{PlatformArch, PlatformInfo, PlatformTimer, SerialConsole, PowerController};
use spin::Mutex;
use x86_64::instructions::port::Port;

/// x86_64 PC/QEMU Mobile Emulation Platform.
pub struct X86_64Platform {
    info: PlatformInfo,
}

impl X86_64Platform {
    pub const fn new() -> Self {
        Self {
            info: PlatformInfo {
                arch: PlatformArch::X86_64,
                soc_name: "QEMU-virt/Mobile-HAL",
                board_name: "InterOS Sovereign Reference DevBoard",
                cpu_cores: 1,
                base_frequency_mhz: 2400,
                total_ram_bytes: 128 * 1024 * 1024, // 128 MiB QEMU memory
                dtb_base: None,
            },
        }
    }

    pub fn info(&self) -> &PlatformInfo {
        &self.info
    }
}

pub struct X86Timer;

impl PlatformTimer for X86Timer {
    fn read_ticks(&self) -> u64 {
        crate::interrupts::get_system_ticks()
    }

    fn frequency_hz(&self) -> u64 {
        100 // 100 Hz PIT timer tick
    }
}

pub struct X86SerialConsole;

impl SerialConsole for X86SerialConsole {
    fn write_byte(&self, byte: u8) {
        use core::fmt::Write;
        crate::arch::with_interrupts_disabled(|| {
            if let Some(ref mut serial) = *crate::serial::SERIAL1.lock() {
                let _ = serial.write_char(byte as char);
            }
        });
    }

    fn read_byte(&self) -> Option<u8> {
        // Non-blocking read on COM1
        let mut port_lsr = Port::<u8>::new(0x3F8 + 5);
        let status = unsafe { port_lsr.read() };
        if (status & 1) != 0 {
            let mut port_data = Port::<u8>::new(0x3F8);
            Some(unsafe { port_data.read() })
        } else {
            None
        }
    }
}

pub struct X86PowerController;

impl PowerController for X86PowerController {
    fn sleep_cpu(&self) {
        x86_64::instructions::hlt();
    }

    fn reset(&self) -> ! {
        unsafe {
            // Pulse CPU reset line via 8042 keyboard controller
            let mut p64 = Port::<u8>::new(0x64);
            p64.write(0xFE);
            // Fallback: PCI system reset register
            let mut pcf9 = Port::<u8>::new(0xCF9);
            pcf9.write(0x06);
        }
        loop {
            x86_64::instructions::hlt();
        }
    }

    fn shutdown(&self) -> ! {
        unsafe {
            // QEMU debug power-off port (ACPI shutdown in microkernel)
            let mut p604 = Port::<u16>::new(0x604);
            p604.write(0x2000);
        }
        loop {
            x86_64::instructions::hlt();
        }
    }
}

/// Global static platform instances.
pub static PLATFORM: X86_64Platform = X86_64Platform::new();
pub static TIMER: X86Timer = X86Timer;
pub static CONSOLE: X86SerialConsole = X86SerialConsole;
pub static POWER: X86PowerController = X86PowerController;
pub static ACTIVE_SOC_NAME: Mutex<&'static str> = Mutex::new("QEMU-virt/Mobile-HAL");
