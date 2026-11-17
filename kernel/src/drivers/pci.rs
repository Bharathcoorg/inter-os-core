// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! PCI Bus Enumeration & Configuration Space Driver
//!
//! Scans PCI configuration space (Ports 0xCF8 / 0xCFC) to detect
//! peripheral controllers, network devices (Ethernet), storage, and bridges.

use alloc::vec::Vec;
use x86_64::instructions::port::Port;
use crate::serial_println;

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// PCI Device Location Identifier
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciAddress {
    pub const fn new(bus: u8, device: u8, function: u8) -> Self {
        Self { bus, device, function }
    }

    /// Computes the 32-bit CONFIG_ADDRESS value for a register offset.
    #[inline]
    fn config_address(&self, offset: u8) -> u32 {
        (1u32 << 31)
            | ((self.bus as u32) << 16)
            | ((self.device as u32) << 11)
            | ((self.function as u32) << 8)
            | ((offset as u32) & 0xFC)
    }
}

/// Discovered PCI Device Information
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciDevice {
    pub addr: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub interrupt_line: u8,
}

impl PciDevice {
    /// Traverses the PCI Capability Linked List to find a capability structure with `target_cap_id`.
    ///
    /// Implements bounded traversal (max 48 hops) to guarantee forward progress and prevent infinite
    /// loops on malformed or corrupted configuration spaces. Returns the configuration space offset
    /// of the matched capability header if found.
    pub fn find_capability(&self, target_cap_id: u8) -> Option<u8> {
        let status = pci_read_u16(self.addr, 0x06);
        // Bit 4 of Status register indicates whether Capabilities List is supported.
        if (status & (1 << 4)) == 0 {
            return None;
        }

        let cap_ptr_raw = pci_read_u8(self.addr, 0x34);
        Self::find_capability_in_space(target_cap_id, cap_ptr_raw, |offset| pci_read_u8(self.addr, offset))
    }

    /// Pure bounded traversal algorithm over PCI configuration space.
    /// Rejects unaligned pointers (ptr & 3 != 0) and out-of-range offsets (< 0x40 or > 0xFC).
    /// Tracks visited DWORD-aligned offsets using a u64 bitmask for O(1) cycle detection.
    pub fn find_capability_in_space<F: Fn(u8) -> u8>(
        target_cap_id: u8,
        initial_cap_ptr: u8,
        read_u8: F,
    ) -> Option<u8> {
        // Capabilities must reside in device-specific config space [0x40, 0xFC] and be DWORD-aligned.
        if initial_cap_ptr & 3 != 0 || initial_cap_ptr < 0x40 || initial_cap_ptr > 0xFC {
            return None;
        }

        let mut cap_ptr = initial_cap_ptr;
        let mut hops = 0;
        let mut visited: u64 = 0;

        while hops < 48 {
            if cap_ptr & 3 != 0 || cap_ptr < 0x40 || cap_ptr > 0xFC {
                return None;
            }

            // Map [0x40, 0xFC] DWORD offsets (steps of 4) into bit index [0, 47]
            let bit_idx = (cap_ptr >> 2) - 16;
            let mask = 1u64 << bit_idx;
            if (visited & mask) != 0 {
                // Cycle detected in capability chain!
                return None;
            }
            visited |= mask;

            let id = read_u8(cap_ptr);
            if id == target_cap_id {
                return Some(cap_ptr);
            }

            let next = read_u8(cap_ptr.saturating_add(1));
            if next == 0 {
                break;
            }
            if next & 3 != 0 || next < 0x40 || next > 0xFC {
                // Out-of-range or unaligned next pointer: reject
                return None;
            }
            cap_ptr = next;
            hops += 1;
        }
        None
    }

    /// Enables PCI memory-space and bus-mastering for a DMA-capable device.
    pub fn enable_memory_and_bus_master(&self) {
        let mut command = pci_read_u16(self.addr, 0x04);
        command |= 1 << 1;
        command |= 1 << 2;
        pci_write_u16(self.addr, 0x04, command);
    }

    /// Reads a memory BAR base address from the device configuration header.
    pub fn memory_bar(&self, index: u8) -> Option<u64> {
        if index >= 6 { return None; }
        let offset = 0x10u8.saturating_add(index.saturating_mul(4));
        let low = pci_read_u32(self.addr, offset);
        if low == 0 || low == 0xFFFF_FFFF { return None; }
        if low & 1 != 0 { return None; }
        let base = (low & 0xFFFF_FFF0) as u64;
        if low & 0x4 != 0 {
            if index >= 5 { return None; }
            let high = pci_read_u32(self.addr, offset + 4) as u64;
            Some(base | (high << 32))
        } else {
            Some(base)
        }
    }


    /// Returns true if this device is an Ethernet Network Controller (Class 0x02, Subclass 0x00).
    pub fn is_network_controller(&self) -> bool {
        self.class_code == 0x02 && self.subclass == 0x00
    }

    /// Returns true if this is an Intel e1000 Gigabit network card (Vendor 0x8086).
    pub fn is_intel_e1000(&self) -> bool {
        self.vendor_id == 0x8086 && (self.device_id == 0x100E || self.device_id == 0x100F || self.device_id == 0x10D3)
    }

    /// Returns true if this is a Realtek RTL8139 network card (Vendor 0x10EC, Device 0x8139).
    pub fn is_realtek_8139(&self) -> bool {
        self.vendor_id == 0x10EC && self.device_id == 0x8139
    }

    /// Returns true if this device is an Audio / Multimedia Controller (Class 0x04).
    pub fn is_audio_controller(&self) -> bool {
        self.class_code == 0x04
    }

    /// Returns true if this is an Intel High Definition Audio controller.
    pub fn is_intel_hda(&self) -> bool {
        self.class_code == 0x04 && (self.subclass == 0x03 || self.device_id == 0x2668)
    }

    /// Returns true if this is an AC'97 audio controller.
    pub fn is_ac97(&self) -> bool {
        self.class_code == 0x04 && (self.subclass == 0x01 || self.device_id == 0x2415 || self.device_id == 0x2445)
    }

    /// Returns true if this device is a Display / VGA Controller (Class 0x03).
    pub fn is_display_controller(&self) -> bool {
        self.class_code == 0x03
    }

    /// Returns true if this is a VirtIO-GPU display adapter (Vendor 0x1AF4, Device 0x1050 or 0x1010).
    pub fn is_virtio_gpu(&self) -> bool {
        self.vendor_id == 0x1AF4 && (self.device_id == 0x1050 || self.device_id == 0x1010)
    }

    /// Returns true if this is a VirtIO device (Vendor 0x1AF4).
    pub fn is_virtio(&self) -> bool {
        self.vendor_id == 0x1AF4
    }
}

/// Reads a 32-bit register from PCI configuration space.
pub fn pci_read_u32(addr: PciAddress, offset: u8) -> u32 {
    let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
    let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

    unsafe {
        addr_port.write(addr.config_address(offset));
        data_port.read()
    }
}

/// Reads a 16-bit value from PCI configuration space.
pub fn pci_read_u16(addr: PciAddress, offset: u8) -> u16 {
    let val32 = pci_read_u32(addr, offset);
    let shift = (offset & 2) * 8;
    ((val32 >> shift) & 0xFFFF) as u16
}

/// Reads an 8-bit value from PCI configuration space.
pub fn pci_read_u8(addr: PciAddress, offset: u8) -> u8 {
    let val32 = pci_read_u32(addr, offset);
    let shift = (offset & 3) * 8;
    ((val32 >> shift) & 0xFF) as u8
}

/// Writes a 32-bit register to PCI configuration space.
pub fn pci_write_u32(addr: PciAddress, offset: u8, value: u32) {
    let mut addr_port = Port::<u32>::new(PCI_CONFIG_ADDRESS);
    let mut data_port = Port::<u32>::new(PCI_CONFIG_DATA);

    unsafe {
        addr_port.write(addr.config_address(offset));
        data_port.write(value);
    }
}

/// Writes a 16-bit value to PCI configuration space using read-modify-write.
pub fn pci_write_u16(addr: PciAddress, offset: u8, value: u16) {
    let aligned_offset = offset & 0xFC;
    let shift = (offset & 2) * 8;
    let current = pci_read_u32(addr, aligned_offset);
    let mask = !(0xFFFFu32 << shift);
    let new_val = (current & mask) | ((value as u32) << shift);
    pci_write_u32(addr, aligned_offset, new_val);
}

/// Scans PCI bus 0 and returns all discovered functional devices.
pub fn scan_pci_bus() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    for bus in 0..=1 {
        for device in 0..32 {
            let addr = PciAddress::new(bus, device, 0);
            let vendor_id = pci_read_u16(addr, 0x00);
            if vendor_id == 0xFFFF || vendor_id == 0x0000 {
                continue; // No device present
            }

            let header_type = pci_read_u8(addr, 0x0E);
            let is_multi_function = (header_type & 0x80) != 0;
            let max_fn = if is_multi_function { 8 } else { 1 };

            for function in 0..max_fn {
                let fn_addr = PciAddress::new(bus, device, function);
                let fn_vendor = pci_read_u16(fn_addr, 0x00);
                if fn_vendor == 0xFFFF || fn_vendor == 0x0000 {
                    continue;
                }

                let device_id = pci_read_u16(fn_addr, 0x02);
                let class_prog = pci_read_u32(fn_addr, 0x08);
                let class_code = ((class_prog >> 24) & 0xFF) as u8;
                let subclass = ((class_prog >> 16) & 0xFF) as u8;
                let prog_if = ((class_prog >> 8) & 0xFF) as u8;
                let fn_header = pci_read_u8(fn_addr, 0x0E) & 0x7F;
                let interrupt_line = pci_read_u8(fn_addr, 0x3C);

                devices.push(PciDevice {
                    addr: fn_addr,
                    vendor_id: fn_vendor,
                    device_id,
                    class_code,
                    subclass,
                    prog_if,
                    header_type: fn_header,
                    interrupt_line,
                });
            }
        }
    }

    devices
}


/// Returns the first discovered Intel E1000-compatible PCI controller.
pub fn find_e1000() -> Option<PciDevice> {
    scan_pci_bus().into_iter().find(|d| d.is_intel_e1000())
}

/// Returns the first discovered VirtIO-GPU PCI display controller if present.
pub fn find_virtio_gpu() -> Option<PciDevice> {
    scan_pci_bus().into_iter().find(|d| d.is_virtio_gpu())
}


/// Attempts to initialize the first Intel E1000 controller discovered on PCI.
pub fn initialize_e1000_network() -> Result<(), &'static str> {
    let device = find_e1000().ok_or("No Intel E1000-compatible controller found")?;
    device.enable_memory_and_bus_master();
    let command = pci_read_u16(device.addr, 0x04);
    if command & (1 << 1) == 0 || command & (1 << 2) == 0 {
        return Err("E1000 PCI memory/bus-mastering could not be enabled");
    }
    let bar = device.memory_bar(0).ok_or("E1000 MMIO BAR0 unavailable")?;
    let mmio_base = x86_64::VirtAddr::new(0xffff_9000_0000_0000);
    let mapped = crate::memory::map_mmio_region(
        x86_64::PhysAddr::new(bar & !0xfff),
        0x20000,
        mmio_base,
    )?;
    let mac = unsafe { crate::net::E1000Device::read_mac_from_eeprom(mapped.as_u64() as usize) };
    if mac == crate::net::MacAddress::ZERO {
        // WHY a zero address is refused rather than replaced with a local
        // address: zero is what an absent or erased EEPROM reads back, so
        // attaching with it would put a forged station address on the
        // gateway's segment. The controller stays unattached and the caller
        // reports the network unavailable instead.
        return Err("E1000 EEPROM returned an invalid zero MAC");
    }
    // WHY check before initialize(): initialize() enables the controller and
    // sets the software link-up bit, at which point the device begins DMA into
    // the descriptor buffers. Binding is gated until that mapping is verified,
    // so the gate must be evaluated before the controller is ever enabled.
    if !crate::net::ENABLE_E1000_BINDING {
        return Err("E1000 binding is disabled pending DMA mapping verification");
    }
    let mut nic = unsafe {
        crate::net::E1000Device::allocate_with_dma(mapped.as_u64() as usize, mac)
            .ok_or("Unable to allocate contiguous E1000 DMA memory")?
    };
    nic.reset();
    nic.initialize().map_err(|_| "E1000 controller initialization failed")?;
    if !nic.read_link_status() {
        return Err("E1000 controller reports link down after initialization");
    }
    if !crate::net::NETWORK_DEVICE.lock().attach_e1000(nic) {
        return Err("E1000 binding was refused by the device manager");
    }
    serial_println!("[NET] Intel E1000 network backend attached: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac.0[0], mac.0[1], mac.0[2], mac.0[3], mac.0[4], mac.0[5]);
    Ok(())
}

/// Initializes and self-tests the PCI configuration space scanner.
pub fn init() {
    let devices = scan_pci_bus();
    serial_println!("--- [PCI Configuration Space Enumeration] ---");
    serial_println!("[OK] Discovered {} PCI hardware devices.", devices.len());

    for dev in &devices {
        serial_println!(
            "  PCI {:02x}:{:02x}.{} -> ID {:04x}:{:04x} (Class {:02x}:{:02x}, IRQ {})",
            dev.addr.bus, dev.addr.device, dev.addr.function,
            dev.vendor_id, dev.device_id, dev.class_code, dev.subclass, dev.interrupt_line
        );
    }

    for dev in &devices {
        if dev.is_network_controller() {
            let bar = dev.memory_bar(0);
            serial_println!("[NET] PCI network controller {:04x}:{:04x} BAR0={:?}", dev.vendor_id, dev.device_id, bar);
        }
    }

    // Self-test: Verify at least one device was found (Host bridge 0x06:0x00 is guaranteed in QEMU)
    assert!(!devices.is_empty(), "PCI bus scan failed: no devices found");
    serial_println!("[TEST] PCI Bus Enumeration: Hardware device scan PASSED.");

    match initialize_e1000_network() {
        Ok(()) => serial_println!("[OK] PCI Ethernet controller attached to canonical network device."),
        Err(e) => serial_println!("[WARN] E1000 network attachment unavailable: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pci_address_config_address() {
        let addr = PciAddress::new(0, 3, 0);
        let cfg = addr.config_address(0x10);
        assert_eq!(cfg & (1 << 31), 1 << 31);
        assert_eq!((cfg >> 16) & 0xFF, 0);
        assert_eq!((cfg >> 11) & 0x1F, 3);
        assert_eq!((cfg >> 8) & 0x7, 0);
        assert_eq!(cfg & 0xFC, 0x10);
    }

    #[test]
    fn test_pci_find_capability_at_first_valid_offset() {
        // Offset 0x40: ID = 0x05 (MSI), Next = 0x00
        let found = PciDevice::find_capability_in_space(0x05, 0x40, |offset| {
            match offset {
                0x40 => 0x05,
                0x41 => 0x00,
                _ => 0,
            }
        });
        assert_eq!(found, Some(0x40));
    }

    #[test]
    fn test_pci_find_capability_at_final_valid_offset() {
        // Offset 0x40 -> 0xFC: ID = 0x10 (PCI Express)
        let found = PciDevice::find_capability_in_space(0x10, 0x40, |offset| {
            match offset {
                0x40 => 0x01, // Power Management
                0x41 => 0xFC, // Next -> 0xFC
                0xFC => 0x10, // Target PCIe
                0xFD => 0x00, // End of list
                _ => 0,
            }
        });
        assert_eq!(found, Some(0xFC));
    }

    #[test]
    fn test_pci_find_capability_unaligned_pointer() {
        // Unaligned initial pointers (ptr & 3 != 0) must be rejected with None
        let found = PciDevice::find_capability_in_space(0x09, 0x43, |offset| {
            match offset {
                0x40 => 0x09,
                0x41 => 0x00,
                _ => 0,
            }
        });
        assert_eq!(found, None);

        assert_eq!(PciDevice::find_capability_in_space(0x09, 0x41, |_| 0x09), None);
        assert_eq!(PciDevice::find_capability_in_space(0x09, 0x42, |_| 0x09), None);
    }

    #[test]
    fn test_pci_find_capability_unaligned_next_pointer() {
        // Unaligned next pointer in chain must be rejected with None
        let found = PciDevice::find_capability_in_space(0x05, 0x40, |offset| {
            match offset {
                0x40 => 0x01,
                0x41 => 0x51, // Unaligned next pointer (0x51 & 3 != 0)
                0x51 => 0x05,
                _ => 0,
            }
        });
        assert_eq!(found, None);
    }

    #[test]
    fn test_pci_find_capability_invalid_pointer() {
        // Initial pointer < 0x40 or > 0xFC is invalid for standard PCI capabilities
        assert_eq!(PciDevice::find_capability_in_space(0x05, 0x30, |_| 0x05), None);
        assert_eq!(PciDevice::find_capability_in_space(0x05, 0x00, |_| 0x05), None);
        assert_eq!(PciDevice::find_capability_in_space(0x05, 0xFD, |_| 0x05), None);
        assert_eq!(PciDevice::find_capability_in_space(0x05, 0xFE, |_| 0x05), None);
    }

    #[test]
    fn test_pci_find_capability_self_loop() {
        // Offset 0x50 has Next = 0x50 (direct self-loop)
        let found = PciDevice::find_capability_in_space(0x99, 0x50, |offset| {
            match offset {
                0x50 => 0x01,
                0x51 => 0x50, // Self-loop
                _ => 0,
            }
        });
        assert_eq!(found, None);
    }

    #[test]
    fn test_pci_find_capability_multi_node_cycle() {
        // Cycle: 0x40 -> 0x50 -> 0x60 -> 0x40
        let found = PciDevice::find_capability_in_space(0x99, 0x40, |offset| {
            match offset {
                0x40 => 0x01,
                0x41 => 0x50,
                0x50 => 0x02,
                0x51 => 0x60,
                0x60 => 0x03,
                0x61 => 0x40, // Back to 0x40
                _ => 0,
            }
        });
        // Must safely terminate after 48 hops without hanging
        assert_eq!(found, None);
    }

    #[test]
    fn test_pci_find_capability_malformed_list() {
        // 0x40 has Next pointing to 0x20 (invalid offset < 0x40)
        let found = PciDevice::find_capability_in_space(0x05, 0x40, |offset| {
            match offset {
                0x40 => 0x01,
                0x41 => 0x20, // Invalid next pointer
                _ => 0,
            }
        });
        assert_eq!(found, None);
    }

    #[test]
    fn test_pci_find_capability_max_traversal_bound() {
        // Chain of 50 nodes where target (0xEE) is placed at node 49 (which exceeds 48 hops)
        let found = PciDevice::find_capability_in_space(0xEE, 0x40, |offset| {
            let hop = (offset.saturating_sub(0x40)) / 4;
            if offset % 2 == 0 {
                if hop == 49 { 0xEE } else { 0x01 }
            } else {
                // Next pointer: next node (simulate chain by wrapping offset)
                0x40u8.wrapping_add(((hop + 1) * 4) as u8)
            }
        });
        // Max 48 hops must terminate before reaching hop 49
        assert_eq!(found, None);
    }
}
