// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! PCIe discovery, driven entirely by the device tree.
//!
//! The bus is only walked when the platform actually declares it. When no ECAM
//! host bridge is present the function reports the bus as unavailable rather
//! than assuming one, so nothing downstream can act on a bus that was never
//! enumerated.

use crate::fdt::ArmFdtParser;
use crate::mmu::ArmMmu;
use crate::pcie::{PcieDevice, PcieEcam};

/// ECAM windows are conventionally this size. Anything the device tree declares
/// is honoured, but an implausibly large window is refused rather than mapped.
const ECAM_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// What the platform exposes on PCIe.
pub struct PcieBus {
    ecam: Option<PcieEcam>,
    devices: [Option<PcieDevice>; 32],
}

impl PcieBus {
    /// A bus that is definitively not present, for when there is no device tree
    /// to interrogate.
    pub const fn unavailable() -> Self {
        Self {
            ecam: None,
            devices: [None; 32],
        }
    }

    /// True when a host bridge was found and mapped.
    pub fn is_available(&self) -> bool {
        self.ecam.is_some()
    }

    /// The devices found on bus 0.
    pub fn devices(&self) -> &[Option<PcieDevice>; 32] {
        &self.devices
    }

    /// Locates the ECAM host bridge declared by the device tree.
    pub fn init_from_fdt(parser: &ArmFdtParser) -> Self {
        let mut bus = Self { ecam: None, devices: [None; 32] };

        let node = match parser.find_node_by_compatible("pci-host-ecam-generic") {
            Ok(node) => node,
            Err(_) => {
                crate::kprintln!(
                    "[PCI] No host bridge matched 'pci-host-ecam-generic'; checking by unit address."
                );
                match parser.find_node_by_name("pcie") {
                    Ok(node) => node,
                    Err(_) => {
                        crate::kprintln!(
                            "[PCI] No PCIe host bridge in the device tree; bus unavailable."
                        );
                        return bus;
                    }
                }
            }
        };

        // `reg` is two cells on a root with 2/2 addressing: base then size.
        let reg = match parser.get_node_property(&node, "reg") {
            Ok(reg) if reg.len() >= 16 => reg,
            _ => {
                crate::kprintln!("[PCI] Host bridge has no usable reg property; bus unavailable.");
                return bus;
            }
        };
        let base = u64::from_be_bytes([
            reg[0], reg[1], reg[2], reg[3], reg[4], reg[5], reg[6], reg[7],
        ]);
        let size = u64::from_be_bytes([
            reg[8], reg[9], reg[10], reg[11], reg[12], reg[13], reg[14], reg[15],
        ]);
        if base == 0 || size == 0 || size > ECAM_MAX_BYTES {
            crate::kprintln!("[PCI] Host bridge reg is out of range; bus unavailable.");
            return bus;
        }

        // Try the coarsest exact mapping first, then the dedicated high table.
        let mapped = ArmMmu::map_device_region(base, size) || ArmMmu::map_device_region_high(base, size);
        if !mapped {
            crate::kprintln!(
                "[PCI] ECAM window could not be mapped (base 0x{:X}, size 0x{:X}); bus unavailable.",
                base,
                size
            );
            return bus;
        }
        crate::kprintln!("[PCI] ECAM window mapped at 0x{:X} ({} MiB).", base, size / (1024 * 1024));

        let ecam = PcieEcam::new(base as usize);
        let devices = unsafe { ecam.enumerate_bus0() };
        let found = devices.iter().filter(|d| d.is_some()).count();
        if found == 0 {
            crate::kprintln!(
                "[PCI] No devices answered on bus 0; the window is mapped but the host bridge"
            );
            crate::kprintln!("[PCI]       returned no configuration data.");
        }
        bus.devices = devices;

        // 16c. Assign the memory BARs the bus walk found unassigned.
        //
        // Nothing on this platform has programmed them: the machine is booted
        // with `-kernel` and no firmware, and the machine model leaves BAR
        // assignment to firmware. Until they are given an address the devices
        // are enumerated but unreachable, so this runs before anything tries to
        // talk to one.
        if found > 0 {
            let mut allocator = crate::pci_bars::BarAllocator::new();
            let (windows, window_count) = crate::pci_bars::parse_ranges(parser, &node);
            allocator.set_windows(windows, window_count);
            allocator.reserve_ecam(base, size);
            // DRAM comes from the device tree, so the set of addresses that must
            // stay clear is the platform's own statement rather than a guess.
            if let Ok(banks) = parser.parse_memory_regions() {
                for bank in banks {
                    allocator.reserve_ram(bank.0, bank.1);
                }
            }
            if !allocator.has_window() {
                crate::kprintln!(
                    "[PCI] The host bridge publishes no usable memory window, so no BAR can be assigned."
                );
            } else {
                for index in 0..window_count {
                    let window = windows[index];
                    // A 64 KiB window reports as 0 MiB, which reads like a bug,
                    // so give the exact byte count alongside it.
                    crate::kprintln!(
                        "[PCI] host bridge range {}: 0x{:X} .. 0x{:X} ({} bytes, {} MiB){}.",
                        index,
                        window.base,
                        window.base.saturating_add(window.size),
                        window.size,
                        window.size / (1024 * 1024),
                        if window.base.saturating_add(window.size) > crate::pci_bars::MAX_REACHABLE {
                            " - above the address space these translation tables describe, so it cannot be used"
                        } else {
                            ""
                        }
                    );
                }
                let mut index = 0usize;
                let mut total = 0u32;
                while let Some(device) = bus.devices.get(index).and_then(|d| *d) {
                    if !device.is_absent() {
                        total += unsafe { crate::pci_bars::assign_device_bars(&ecam, &device, &mut allocator) };
                    }
                    index += 1;
                }
                crate::kprintln!("[PCI] {} memory BAR(s) assigned from the host bridge ranges.", total);
            }
        }
        bus.ecam = Some(ecam);

        // Negotiate with any virtio device the platform actually exposes.
        let mut index = 0usize;
        while let Some(device) = bus.devices.get(index).and_then(|d| *d) {
            if crate::virtio::VirtioPciDevice::is_virtio(&device) {
                let ecam_ref = bus.ecam.as_ref();
                match unsafe { crate::virtio::VirtioPciDevice::probe(ecam_ref.unwrap(), device) } {
                    Some(virtio) => {
                        let queues = unsafe { virtio.queue_count() };
                        crate::kprintln!(
                            "[VIRTIO] {:04x}:{:04x} negotiated virtio 1.0, features 0x{:016X}, {} queue(s), notify bar 0x{:X}",
                            device.vendor_id, device.device_id, virtio.features, queues, virtio.notify_bar
                        );
                        if unsafe { virtio.begin_init() } {
                            unsafe { virtio.finish_init() };
                            // ACK | DRIVER | FEATURES_OK | DRIVER_OK is 15. Reading
                            // it back is what proves the device saw each step.
                            let status = unsafe { virtio.device_status() };
                            crate::kprintln!(
                                "[VIRTIO] {:04x}:{:04x} initialised, device status 0x{:02X} (expect 0x0F).",
                                device.vendor_id, device.device_id, status
                            );
                            unsafe { report_device_config(&virtio, &device) };
                        } else {
                            crate::kprintln!("[VIRTIO] {:04x}:{:04x} refused the handshake.", device.vendor_id, device.device_id);
                        }
                    }
                    None => {
                        crate::kprintln!(
                            "[VIRTIO] {:04x}:{:04x} could not be claimed: one of the BARs its \
                             capabilities refer to has no assigned address.",
                            device.vendor_id, device.device_id
                        );
                    }
                }
            }
            index += 1;
        }
        bus
    }

    /// The ECAM accessor, when the bus is available.
    pub fn ecam(&self) -> Option<&PcieEcam> {
        self.ecam.as_ref()
    }

    /// Enables memory space and bus mastering on a device and returns its BAR 0
    /// physical address. Returns `None` when the device is absent or the bus is
    /// unavailable, so a caller cannot act on an unenumerated device.
    pub fn map_first_bar(&self, slot: usize) -> Option<u64> {
        let ecam = self.ecam.as_ref()?;
        let device = self.devices.get(slot).and_then(|d| *d)?;
        if device.is_absent() {
            return None;
        }
        unsafe {
            ecam.enable_memory_and_bus_master(0, device.device, device.function);
            ecam.read_bar64(0, device.device, device.function, 0)
        }
    }
}

/// The device identifier virtio-gpu presents, which is the display controller.
const VIRTIO_GPU_DEVICE: u16 = 0x1050;

/// Reads and reports the leading words of a device's configuration space.
///
/// The virtio specification only lets a driver read configuration space once
/// the device has seen `DRIVER`, so this runs after the handshake. The field
/// names are only applied to virtio-gpu, whose layout is known; for any other
/// device the raw words are printed instead, because naming them after another
/// device's structure would be a claim this driver cannot support.
///
/// # Safety
/// The configuration capability's BAR must already be mapped.
unsafe fn report_device_config(virtio: &crate::virtio::VirtioPciDevice, device: &PcieDevice) {
    if !virtio.has_device_config() {
        crate::kprintln!(
            "[VIRTIO] {:04x}:{:04x} published no configuration capability.",
            device.vendor_id, device.device_id
        );
        return;
    }
    const WORDS: usize = 5;
    let gpu = device.device_id == VIRTIO_GPU_DEVICE;
    let names: [&str; WORDS] = ["events_read", "cursor_x", "cursor_y", "width", "height"];
    let label = if gpu { "virtio-gpu" } else { "unnamed virtio device" };
    let mut line = crate::alloc::format!("[VIRTIO] {} configuration space:", label);
    let mut readable = true;
    for index in 0..WORDS {
        match virtio.device_config_u32(index) {
            Some(value) => {
                if gpu {
                    line.push_str(&crate::alloc::format!(" {}={}", names[index], value));
                } else {
                    line.push_str(&crate::alloc::format!(" word{}={}", index, value));
                }
            }
            None => readable = false,
        }
    }
    if readable {
        crate::kprintln!("{}", line);
        if gpu {
            crate::kprintln!(
                "[VIRTIO] virtio-gpu reports width={} height={}; a zero pair means the device published \
                 no display mode, so this is the configuration read and nothing more.",
                virtio.device_config_u32(3).unwrap_or(0),
                virtio.device_config_u32(4).unwrap_or(0)
            );
        }
    } else {
        crate::kprintln!(
            "[VIRTIO] {} published a configuration capability but the read failed.",
            label
        );
    }
}
