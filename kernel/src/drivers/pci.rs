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


/// Attempts to initialize the first Intel E1000 controller discovered on PCI.
pub fn initialize_e1000_network() -> Result<(), &'static str> {
    let device = find_e1000().ok_or("No Intel E1000-compatible controller found")?;
    let bar = device.memory_bar(0).ok_or("E1000 MMIO BAR0 unavailable")?;
    let mmio_base = crate::memory::VirtAddr::new(0xffff_9000_0000_0000);
    let mapped = crate::memory::map_mmio_region(
        crate::memory::PhysAddr::new(bar & !0xfff),
        0x20000,
        mmio_base,
    )?;
    let mac = unsafe { crate::net::E1000Device::read_mac_from_eeprom(mapped.as_u64() as usize) };
    if mac == crate::net::MacAddress::ZERO {
        return Err("E1000 EEPROM returned an invalid zero MAC");
    }
    let mut nic = unsafe {
        crate::net::E1000Device::allocate_with_dma(mapped.as_u64() as usize, mac)
            .ok_or("Unable to allocate contiguous E1000 DMA memory")?
    };
    nic.initialize().map_err(|_| "E1000 controller initialization failed")?;
    crate::net::NETWORK_DEVICE.lock().attach_e1000(nic);
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
