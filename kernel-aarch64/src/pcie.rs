//! PCIe Configuration Space Access via ECAM (Enhanced Configuration Access Mechanism)
//!
//! QEMU's `virt` machine has no legacy 0xCF8/0xCFC ports, so a display device
//! can only be reached by mapping the ECAM window that the device tree declares
//! at `pcie@10000000` and walking the configuration space through it.
//!
//! This module deliberately stops at enumeration. It exists so a bus driver can
//! discover which devices the platform actually exposes instead of assuming an
//! address, and so the transport can be verified on its own.

use core::ptr::{read_volatile, write_volatile};

/// ECAM MMIO base. The physical address is taken from the device tree node that
/// declares `compatible = "pci-host-ecam-generic"`, so nothing is hardcoded.
pub struct PcieEcam {
    base: usize,
}

/// One 4096-byte register window per bus/device/function.
const ECAM_BUS_STRIDE: usize = 0x1_0000;
/// Each function occupies 256 bytes of configuration space.
const ECAM_FUNCTION_STRIDE: usize = 0x100;

/// Config space header offsets.
const PCI_VENDOR_ID: usize = 0x00;
const PCI_COMMAND: usize = 0x04;
const PCI_CLASS_REVISION: usize = 0x08;
const PCI_BAR0: usize = 0x10;

/// A device found on the bus.
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
    /// True when the vendor field reads back as 0xFFFF, which is how an absent
    /// function presents itself.
    pub fn is_absent(&self) -> bool {
        self.vendor_id == 0xFFFF
    }

    /// Human-readable bus address, e.g. `0000:00:02.0`.
    pub fn address(&self) -> (u16, u8, u8) {
        (self.bus, self.device, self.function)
    }
}

impl PcieEcam {
    /// Creates an accessor over an already-mapped ECAM window.
    ///
    /// The caller is responsible for having mapped the window; this type never
    /// invents a physical address.
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    fn config_ptr(&self, bus: u16, device: u8, function: u8) -> *const u8 {
        // ECAM layout: 64 KiB per bus, then 256 bytes per function, with the
        // eight functions of a device slot contiguous. Treating the device
        // number's high bits as a bus index is a common mistake and silently
        // makes every device look absent.
        let offset = (bus as usize) * ECAM_BUS_STRIDE
            + (((device as usize) * 8) + (function as usize)) * ECAM_FUNCTION_STRIDE;
        (self.base + offset) as *const u8
    }

    /// Reads a 32-bit config space word.
    ///
    /// # Safety
    /// The ECAM window must be mapped and the address must be in range.
    pub unsafe fn read32(&self, bus: u16, device: u8, function: u8, offset: usize) -> u32 {
        read_volatile(self.config_ptr(bus, device, function).add(offset) as *const u32)
    }

    /// Writes a 32-bit config space word.
    ///
    /// # Safety
    /// As [`PcieEcam::read32`], and the caller must be authorised to change the
    /// device.
    pub unsafe fn write32(
        &self,
        bus: u16,
        device: u8,
        function: u8,
        offset: usize,
        value: u32,
    ) {
        write_volatile(
            self.config_ptr(bus, device, function).add(offset) as *mut u32,
            value,
        )
    }

    /// Reads the identifying header of one function.
    ///
    /// # Safety
    /// As [`PcieEcam::read32`].
    pub unsafe fn probe(
        &self,
        bus: u16,
        device: u8,
        function: u8,
    ) -> PcieDevice {
        let id = self.read32(bus, device, function, PCI_VENDOR_ID);
        let class_revision = self.read32(bus, device, function, PCI_CLASS_REVISION);
        PcieDevice {
            bus,
            device,
            function,
            vendor_id: (id & 0xFFFF) as u16,
            device_id: (id >> 16) as u16,
            class_code: class_revision >> 8,
        }
    }

    /// Enumerates bus 0, walking all 32 slots and single-function devices.
    ///
    /// Multi-function devices are followed through the header type byte; a
    /// header type of 0xFF means the rest of that slot is empty.
    ///
    /// # Safety
    /// The ECAM window must be mapped.
    pub unsafe fn enumerate_bus0(&self) -> [Option<PcieDevice>; 32] {
        let mut found: [Option<PcieDevice>; 32] = [None; 32];
        for device in 0u8..32 {
            let first = self.probe(0, device, 0);
            if first.is_absent() {
                continue;
            }
            found[device as usize] = Some(first);
            // Follow the multi-function chain if the device advertises it.
            let header_type = self.read32(0, device, 0, 0x0C) & 0x80;
            if header_type != 0 {
                for function in 1u8..8 {
                    let extra = self.probe(0, device, function);
                    if extra.is_absent() {
                        break;
                    }
                    if let Some(slot) = found.get_mut(device as usize) {
                        *slot = Some(extra);
                    }
                }
            }
        }
        found
    }

    /// Reads a 64-bit Base Address Register from a device.
    ///
    /// # Safety
    /// As [`PcieEcam::read32`].
    pub unsafe fn read_bar64(
        &self,
        bus: u16,
        device: u8,
        function: u8,
        bar: usize,
    ) -> Option<u64> {
        let low = self.read32(bus, device, function, PCI_BAR0 + bar * 8);
        // A 64-bit memory BAR has bit 0 clear; a 32-bit one has bit 0 set.
        if low & 1 != 0 {
            return Some(low as u32 as u64 & 0xFFFF_FFFC);
        }
        let high = self.read32(bus, device, function, PCI_BAR0 + bar * 8 + 4);
        Some(((high as u64) << 32) | (low as u64 & 0xFFFF_FFFC))
    }

    /// Enables memory space and bus mastering in the command register.
    ///
    /// # Safety
    /// As [`PcieEcam::read32`].
    pub unsafe fn enable_memory_and_bus_master(
        &self,
        bus: u16,
        device: u8,
        function: u8,
    ) {
        let command = self.read32(bus, device, function, PCI_COMMAND);
        // Bit 1: memory space enable. Bit 2: bus master enable.
        let wanted = command | (1 << 1) | (1 << 2);
        if wanted != command {
            self.write32(bus, device, function, PCI_COMMAND, wanted);
        }
    }
}