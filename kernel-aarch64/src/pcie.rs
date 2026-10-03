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
const ECAM_BUS_STRIDE: usize = 0x10_0000;
/// ECAM gives each function a 4 KiB window and each device 8 of them, which is
/// 32 KiB. These are not the 256-byte windows of the legacy 0xCF8/0xCFC
/// mechanism, and using those strides reads the wrong register everywhere.
const ECAM_FUNCTION_STRIDE: usize = 0x1000;

/// Config space header offsets.
const PCI_VENDOR_ID: usize = 0x00;
const PCI_COMMAND: usize = 0x04;
const PCI_CLASS_REVISION: usize = 0x08;
const PCI_BAR0: usize = 0x10;

/// Bits 0..3 of a memory BAR are read-only type bits: bit 0 says the BAR
/// decodes memory rather than port I/O, bit 2 marks a 64-bit BAR and bit 3
/// marks a prefetchable one. They must be cleared out of a size mask before
/// the mask is inverted, or the recovered size is wrong by up to 15 bytes.
const BAR_TYPE_MASK: u32 = 0xF;

/// A Base Address Register after it has been size-probed.
#[derive(Copy, Clone, Debug)]
pub struct BarProbe {
    /// The BAR decodes memory. A port I/O BAR is never assigned here because
    /// this platform publishes no I/O window for it.
    pub is_memory: bool,
    /// The BAR is a 64-bit register pair, so its address spans both dwords.
    pub is_64bit: bool,
    /// The decoded size in bytes.
    pub size: u64,
    /// The address currently programmed, with the type bits cleared.
    pub address: u64,
}

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
        // ECAM layout: 1 MiB per bus, 32 KiB per device slot, and 4 KiB per
        // function within the slot.
        let offset = (bus as usize) * ECAM_BUS_STRIDE
            + (((device as usize) * 8) + (function as usize)) * ECAM_FUNCTION_STRIDE;
        (self.base + offset) as *const u8
    }

    /// Reads a single configuration space byte.
    ///
    /// Capability structures place their type and BAR fields on byte boundaries
    /// that a 32-bit access would straddle, so they have to be read as bytes.
    ///
    /// # Safety
    /// As [`PcieEcam::read32`].
    pub unsafe fn read8(&self, bus: u16, device: u8, function: u8, offset: usize) -> u8 {
        read_volatile(self.config_ptr(bus, device, function).add(offset))
    }

    /// Reads a 32-bit config space word.
    ///
    /// # Safety
    /// The ECAM window must be mapped and the address must be in range, and the
    /// offset must be 4-byte aligned.
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

    /// Size-probes one Base Address Register and leaves it as it was found.
    ///
    /// The size is recovered the way the specification requires: write all ones
    /// to the register, read back the mask, invert it, and add one. The address
    /// that was programmed is restored afterwards, so probing never changes the
    /// device.
    ///
    /// WHY only the low dword decides the width: for a 32-bit BAR the dword
    /// above it is the *next* BAR, so reading a size out of it would report the
    /// neighbour's size and writing to it would corrupt the neighbour. A BAR is
    /// therefore treated as 64-bit only when its own low half cannot express the
    /// size at all, which can only happen for a region of 4 GiB or more. Every
    /// other BAR can be programmed through its low half alone, which is correct
    /// for a 32-bit BAR and for a 64-bit BAR that fits below 4 GiB.
    ///
    /// WHY the type bits are masked out first: bits 0..3 are read-only, so
    /// inverting a mask that still contains them would produce a size that is
    /// wrong by up to 15 bytes.
    ///
    /// # Safety
    /// As [`PcieEcam::read32`].
    pub unsafe fn probe_bar(
        &self,
        bus: u16,
        device: u8,
        function: u8,
        bar: usize,
    ) -> BarProbe {
        let offset = PCI_BAR0 + bar * 4;
        let assigned_low = self.read32(bus, device, function, offset);
        let assigned_high = self.read32(bus, device, function, offset + 4);

        self.write32(bus, device, function, offset, u32::MAX);
        let mask_low = self.read32(bus, device, function, offset);
        self.write32(bus, device, function, offset + 4, u32::MAX);
        let mask_high = self.read32(bus, device, function, offset + 4);
        self.write32(bus, device, function, offset, assigned_low);
        self.write32(bus, device, function, offset + 4, assigned_high);

        let is_memory = mask_low & 1 == 0;
        // A BAR that reads back an all-zero mask is not implemented: the device
        // ignored the write. Its size is zero, which callers must treat as
        // "nothing here" rather than "unbounded".
        if mask_low == 0 {
            return BarProbe {
                is_memory: false,
                is_64bit: false,
                size: 0,
                address: 0,
            };
        }
        let mask_low = mask_low & !BAR_TYPE_MASK;
        if mask_low == 0xFFFF_FFF0 {
            // Every address bit is writable, so the size lives in the upper
            // dword and the BAR is necessarily 64 bits wide.
            let mask_high = mask_high & !BAR_TYPE_MASK;
            let inverted = (((!mask_high) as u64) << 32) | ((!mask_low) as u64);
            return BarProbe {
                is_memory,
                is_64bit: true,
                size: inverted.wrapping_add(1),
                address: (((assigned_high as u64) << 32)
                    | (assigned_low as u64 & !BAR_TYPE_MASK as u64))
                    & 0xFFFF_FFFF_FFFF_FFF0,
            };
        }

        let size = (((!mask_low) as u64).wrapping_add(1)) & 0xFFFF_FFFF;
        BarProbe {
            is_memory,
            is_64bit: false,
            size,
            address: (assigned_low as u64) & 0xFFFF_FFFF_FFFF_FFF0 & !BAR_TYPE_MASK as u64,
        }
    }

    /// Programs the address of an already-assigned BAR and returns the value the
    /// register now reads back.
    ///
    /// Only the low dword is written unless the probe reported a 64-bit BAR, for
    /// the reason given on [`PcieEcam::probe_bar`].
    ///
    /// # Safety
    /// As [`PcieEcam::read32`], and `address` must satisfy the alignment and
    /// size the probe reported.
    pub unsafe fn write_bar_address(
        &self,
        bus: u16,
        device: u8,
        function: u8,
        bar: usize,
        address: u64,
        is_64bit: bool,
    ) -> u64 {
        let offset = PCI_BAR0 + bar * 4;
        if is_64bit {
            self.write32(bus, device, function, offset, address as u32);
            self.write32(bus, device, function, offset + 4, (address >> 32) as u32);
        } else {
            self.write32(bus, device, function, offset, address as u32);
        }
        self.probe_bar(bus, device, function, bar).address
    }
}