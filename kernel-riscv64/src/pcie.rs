// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 PCIe Configuration Space Access via ECAM
//!
//! Provides PCI enumeration and device configuration on RISC-V virt machine.
//! Supports detection and configuration of the Bochs VBE display adapter.

use core::ptr::{read_volatile, write_volatile};

pub const DEFAULT_RISCV_ECAM_BASE: usize = 0x3000_0000;
const ECAM_BUS_STRIDE: usize = 0x10_0000;
const ECAM_FUNCTION_STRIDE: usize = 0x1000;

const PCI_VENDOR_ID: usize = 0x00;
const PCI_COMMAND: usize = 0x04;
const PCI_BAR0: usize = 0x10;

#[derive(Copy, Clone, Debug)]
pub struct PcieDevice {
    pub bus: u16,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u32,
}

impl PcieDevice {
    pub fn is_absent(&self) -> bool {
        self.vendor_id == 0xFFFF
    }
}

pub struct PcieEcam {
    base: usize,
}

impl PcieEcam {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    fn config_ptr(&self, bus: u16, device: u8, function: u8) -> *const u8 {
        let offset = (bus as usize) * ECAM_BUS_STRIDE
            + (((device as usize) * 8) + (function as usize)) * ECAM_FUNCTION_STRIDE;
        (self.base + offset) as *const u8
    }

    pub unsafe fn read32(&self, bus: u16, device: u8, function: u8, reg_offset: usize) -> u32 {
        let ptr = self.config_ptr(bus, device, function).add(reg_offset) as *const u32;
        read_volatile(ptr)
    }

    pub unsafe fn write32(&self, bus: u16, device: u8, function: u8, reg_offset: usize, value: u32) {
        let ptr = self.config_ptr(bus, device, function).add(reg_offset) as *mut u32;
        write_volatile(ptr, value);
    }

    pub unsafe fn enable_memory_and_bus_master(&self, bus: u16, device: u8, function: u8) {
        let command = self.read32(bus, device, function, PCI_COMMAND);
        let wanted = command | (1 << 1) | (1 << 2);
        if wanted != command {
            self.write32(bus, device, function, PCI_COMMAND, wanted);
        }
    }

    pub unsafe fn read_bar_addr(&self, bus: u16, device: u8, function: u8, bar_idx: usize) -> Option<u64> {
        let low = self.read32(bus, device, function, PCI_BAR0 + bar_idx * 4);
        if low == 0 || low == 0xFFFF_FFFF {
            return None;
        }
        if (low & 1) == 0 && (low & 0x6) == 0x4 {
            let high = self.read32(bus, device, function, PCI_BAR0 + bar_idx * 4 + 4);
            Some(((high as u64) << 32) | (low as u64 & 0xFFFF_FFF0))
        } else {
            Some(low as u64 & 0xFFFF_FFF0)
        }
    }

    pub unsafe fn probe(&self, bus: u16, device: u8, function: u8) -> PcieDevice {
        let id = self.read32(bus, device, function, PCI_VENDOR_ID);
        let class_rev = self.read32(bus, device, function, 0x08);
        PcieDevice {
            bus,
            device,
            function,
            vendor_id: (id & 0xFFFF) as u16,
            device_id: (id >> 16) as u16,
            class_code: class_rev >> 8,
        }
    }
}

pub fn probe_and_init_display() {
    let ecam = PcieEcam::new(DEFAULT_RISCV_ECAM_BASE);
    for dev_num in 0u8..32 {
        let dev = unsafe { ecam.probe(0, dev_num, 0) };
        if dev.is_absent() {
            continue;
        }
        if dev.vendor_id == 0x1234 && dev.device_id == 0x1111 {
            unsafe {
                let mut bar0 = ecam.read_bar_addr(0, dev_num, 0, 0);
                let mut bar2 = ecam.read_bar_addr(0, dev_num, 0, 2);

                if bar0.is_none() || bar0 == Some(0) {
                    ecam.write32(0, dev_num, 0, PCI_BAR0, 0x5000_0000);
                    bar0 = Some(0x5000_0000);
                }
                if bar2.is_none() || bar2 == Some(0) {
                    ecam.write32(0, dev_num, 0, PCI_BAR0 + 8, 0x5200_0000);
                    bar2 = Some(0x5200_0000);
                }

                ecam.enable_memory_and_bus_master(0, dev_num, 0);

                if let (Some(b0), Some(b2)) = (bar0, bar2) {
                    crate::display::init_bochs_display(b0 as usize, b2 as usize);
                    return;
                }
            }
        }
    }
}
