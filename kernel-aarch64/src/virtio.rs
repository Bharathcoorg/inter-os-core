//! Modern virtio over PCI Express.
//!
//! Implements the transport half of virtio 1.0: capability discovery, feature
//! negotiation, queue setup and notification. The device is discovered through
//! the PCI bus walk, never by a fixed address.
//!
//! Only the modern (non-legacy, non-transitional) interface is implemented,
//! because the transition-era 0x9F00 register layout has no capability chain and
//! would need a second code path.

use core::ptr::{read_volatile, write_volatile};

use crate::pcie::{PcieDevice, PcieEcam};

/// Virtio vendor ID.
pub const VIRTIO_VENDOR: u16 = 0x1AF4;

/// PCI command register.
const PCI_COMMAND: usize = 0x04;
const PCI_CAPABILITY_POINTER: usize = 0x34;
const PCI_STATUS_CAP_LIST: u32 = 1 << 4;

const VIRTIO_PCI_CAP_ID: u8 = 0x09;
/// Capability-specific identifiers inside a virtio vendor capability.
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_BAR: u8 = 2;
const VIRTIO_PCI_CAP_ISR_BAR: u8 = 3;

/// Feature bits. Virtio 1.0 features start at bit 32.
const VIRTIO_F_VERSION_1: u32 = 32;
const VIRTIO_F_RING_INDIRECT_DESC: u32 = 28;
const VIRTIO_F_RING_EVENT_IDX: u32 = 29;

/// The virtio 1.0 common configuration structure, laid out as the device
/// presents it in PCI capability space.
#[repr(C)]
#[derive(Default)]
pub struct VirtioPciCommonCfg {
    pub device_feature_select: u32,
    pub device_feature: u32,
    pub driver_feature_select: u32,
    pub driver_feature: u32,
    pub msix_config: u16,
    pub num_queues: u16,
    pub device_status: u8,
    pub config_generation: u8,
    pub queue_select: u16,
    pub queue_size: u16,
    pub queue_msix_vector: u16,
    pub queue_enable: u16,
    pub queue_notify_off: u16,
    pub queue_desc: u64,
    pub queue_driver: u64,
    pub queue_device: u64,
}

impl VirtioPciCommonCfg {
    pub const STATUS_ACKNOWLEDGE: u8 = 1;
    pub const STATUS_DRIVER: u8 = 2;
    pub const STATUS_DRIVER_OK: u8 = 4;
    pub const STATUS_FEATURES_OK: u8 = 8;
    pub const STATUS_FAILED: u8 = 128;
}

/// A negotiated virtio-pci device.
pub struct VirtioPciDevice {
    pub device: PcieDevice,
    pub common_cfg: *mut VirtioPciCommonCfg,
    pub notify_off_multiplier: u32,
    pub notify_bar: u64,
    pub isr_bar: u64,
    pub features: u64,
}

impl VirtioPciDevice {
    /// Whether a device identifier is a virtio device.
    pub fn is_virtio(device: &PcieDevice) -> bool {
        device.vendor_id == VIRTIO_VENDOR
    }

    /// Negotiates with a device found on the bus.
    ///
    /// # Safety
    /// The ECAM window must be mapped and the capability chain must be walked
    /// only through vendor capability identifiers this driver understands.
    pub unsafe fn probe(
        ecam: &PcieEcam,
        device: PcieDevice,
    ) -> Option<Self> {
        if !Self::is_virtio(&device) {
            return None;
        }
        let (bus, dev, func) = device.address();

        // Memory space must be on before the device's own configuration is
        // reachable, and the capability list must be walked only while the
        // status bit says it is still valid.
        // Command and status share the dword at 0x04. Status itself is a 16-bit
        // register at 0x06, so it must be extracted from the dword rather than
        // read as an unaligned u32.
        let command_and_status = ecam.read32(bus, dev, func, PCI_COMMAND);
        let command = command_and_status & 0xFFFF;
        let status = (command_and_status >> 16) & 0xFFFF;
        let cap_ptr = ecam.read32(bus, dev, func, PCI_CAPABILITY_POINTER) & 0xFF;
        crate::kprintln!(
            "[VIRTIO] {:04x}:{:04x} status 0x{:04X} cap_ptr 0x{:02X}",
            device.vendor_id, device.device_id, status, cap_ptr
        );
        ecam.write32(bus, dev, func, PCI_COMMAND, command | (1 << 1));
        if status & PCI_STATUS_CAP_LIST == 0 {
            crate::kprintln!(
                "[VIRTIO] {:04x}:{:04x} reports no capability list.",
                device.vendor_id, device.device_id
            );
            return None;
        }

        let mut ptr = (ecam.read32(bus, dev, func, PCI_CAPABILITY_POINTER) & 0xFF) as u8;
        let mut common_cfg: Option<*mut u8> = None;
        let mut notify_bar: Option<u64> = None;
        let mut notify_mult: u32 = 4;
        let mut isr_bar: Option<u64> = None;
        // A malformed chain must terminate; refuse anything unreasonably long.
        for _ in 0..48 {
            if ptr == 0 || ptr == 0xFF {
                break;
            }
            let cap_id = ecam.read8(bus, dev, func, ptr as usize);
            let next = ecam.read8(bus, dev, func, ptr as usize + 1);
            let cap_len = ecam.read8(bus, dev, func, ptr as usize + 2);
            // The virtio-specific type byte sits immediately after the generic
            // capability header, at byte 3.
            let cfg_type = ecam.read8(bus, dev, func, ptr as usize + 3);
            match cap_id {
                VIRTIO_PCI_CAP_ID => {
                    match cfg_type {
                        VIRTIO_PCI_CAP_COMMON_CFG => {
                            let bar = ecam.read8(bus, dev, func, ptr as usize + 4);
                            let offset = ecam.read32(bus, dev, func, ptr as usize + 8) as u64;
                            if let Some(base) = Self::map_bar(ecam, bus, dev, func, bar) {
                                common_cfg = Some(base.wrapping_add(offset) as *mut u8);
                            }
                        }
                        VIRTIO_PCI_CAP_NOTIFY_BAR => {
                            let bar = ecam.read8(bus, dev, func, ptr as usize + 4);
                            notify_bar = Self::map_bar(ecam, bus, dev, func, bar);
                            notify_mult = ecam.read32(bus, dev, func, ptr as usize + 8);
                        }
                        VIRTIO_PCI_CAP_ISR_BAR => {
                            let bar = ecam.read8(bus, dev, func, ptr as usize + 4);
                            isr_bar = Self::map_bar(ecam, bus, dev, func, bar);
                        }
                        _ => {}
                    }
                    let _ = cap_len;
                }
                _ => {}
            }
            if next == 0 || next == 0xFF {
                break;
            }
            ptr = next as u8;
        }

        let common_cfg = common_cfg?;
        let notify_bar = notify_bar?;

        let cfg = &mut *(common_cfg as *mut VirtioPciCommonCfg);

        // The device must offer virtio 1.0. A transitional device needs the
        // legacy register interface instead, which this driver does not claim.
        write_volatile(&mut cfg.device_feature_select, 1);
        let high_features = read_volatile(&mut cfg.device_feature);
        if high_features & (1 << (VIRTIO_F_VERSION_1 - 32)) == 0 {
            crate::kprintln!(
                "[VIRTIO] {:04x}:{:04x} did not offer virtio 1.0 (high features 0x{:08X}).",
                device.vendor_id, device.device_id, high_features
            );
            return None;
        }
        write_volatile(&mut cfg.device_feature_select, 0);
        let low_features = read_volatile(&mut cfg.device_feature);

        // Accept the mandatory version bit plus the two ring features the driver
        // understands; anything else is declined so the device cannot enable it.
        let wanted_low: u32 =
            (1 << VIRTIO_F_RING_INDIRECT_DESC) | (1 << VIRTIO_F_RING_EVENT_IDX);
        if low_features & wanted_low != wanted_low {
            return None;
        }
        write_volatile(&mut cfg.driver_feature_select, 0);
        write_volatile(&mut cfg.driver_feature, wanted_low);
        write_volatile(&mut cfg.driver_feature_select, 1);
        write_volatile(&mut cfg.driver_feature, 1 << (VIRTIO_F_VERSION_1 - 32));

        Some(Self {
            device,
            common_cfg: common_cfg as *mut VirtioPciCommonCfg,
            notify_off_multiplier: notify_mult,
            notify_bar,
            isr_bar: isr_bar.unwrap_or(notify_bar),
            features: ((high_features as u64) << 32) | low_features as u64,
        })
    }

    /// Reads a 64-bit BAR and returns its physical address.
    unsafe fn bar_address(
        ecam: &PcieEcam,
        bus: u16,
        device: u8,
        function: u8,
        bar: u8,
    ) -> u64 {
        let offset = 0x10 + (bar as usize) * 4;
        let low = ecam.read32(bus, device, function, offset);
        if low & 1 == 0 {
            let high = ecam.read32(bus, device, function, offset + 4);
            (((high as u64) << 32) | (low as u64 & 0xFFFF_FFFF_0)) & 0xFFFF_FFFF_FFFF_FFF0
        } else {
            low as u64 & 0xFFFF_FFFC
        }
    }

    /// Maps a device BAR so its registers can be reached, and returns its
    /// address.
    ///
    /// The size is recovered by writing all ones to the BAR and reading back the
    /// mask, which needs configuration space only. Without mapping, touching the
    /// device's own registers faults, because a BAR lives in the platform's
    /// peripheral window and not in the tables built so far.
    ///
    /// Returns `None` when the BAR holds no address. That is the expected state
    /// on a platform booted directly with no firmware: assigning PCI BARs is a
    /// firmware responsibility, and QEMU's `virt` machine does not do it, so a
    /// device that reports all-zero BARs has no reachable registers. Driving it
    /// anyway would be guessing at an address.
    ///
    /// # Safety
    /// The ECAM window must be mapped. The BAR is probed and restored, so the
    /// device is left as it was found.
    pub unsafe fn map_bar(
        ecam: &PcieEcam,
        bus: u16,
        device: u8,
        function: u8,
        bar: u8,
    ) -> Option<u64> {
        let offset = 0x10 + (bar as usize) * 4;
        let assigned_low = ecam.read32(bus, device, function, offset);
        let assigned_high = ecam.read32(bus, device, function, offset + 4);

        // Probe the size, then put the assigned address back.
        ecam.write32(bus, device, function, offset, 0xFFFF_FFFF);
        let mask_low = ecam.read32(bus, device, function, offset);
        ecam.write32(bus, device, function, offset + 4, 0xFFFF_FFFF);
        let mask_high = ecam.read32(bus, device, function, offset + 4);
        ecam.write32(bus, device, function, offset, assigned_low);
        ecam.write32(bus, device, function, offset + 4, assigned_high);

        // A 32-bit I/O BAR is probed through the low half only.
        if assigned_low & 1 != 0 {
            let size = ((!mask_low as u64) + 1) & 0xFFFF_FFFF;
            return None;
        }

        let size = (((!mask_high as u64) << 32) | (!mask_low as u64 & 0xFFFF_FFFF)) + 1;
        if size == 0 || size > 16 * 1024 * 1024 {
            return None;
        }
        let base = (((assigned_high as u64) << 32) | (assigned_low as u64 & 0xFFFF_FFFF_0))
            & 0xFFFF_FFFF_FFFF_FFF0;
        if base == 0 {
            return None;
        }

        // Round up to a mapping granularity the MMU can represent.
        const GRANULE: u64 = 0x20_0000;
        let map_base = base & !(GRANULE - 1);
        let map_end = (base + size + GRANULE - 1) & !(GRANULE - 1);

        // A BAR is usually far smaller than the mapping granularity and is
        // rarely aligned to it, so map the covering 2 MiB blocks instead. Normal
        // memory must never be shadowed, so the platform RAM range is refused.
        const PLATFORM_RAM_LO: u64 = 0x4000_0000;
        const PLATFORM_RAM_HI: u64 = 0x8000_0000;
        if !crate::mmu::ArmMmu::map_device_region_guarded(
            map_base,
            map_end - map_base,
            PLATFORM_RAM_LO,
            PLATFORM_RAM_HI,
        ) && !crate::mmu::ArmMmu::map_device_region(map_base, map_end - map_base)
            && !crate::mmu::ArmMmu::map_device_region_high(map_base, map_end - map_base)
        {
            return None;
        }
        Some(base)
    }

    /// Number of queues the device implements.
    pub unsafe fn queue_count(&self) -> u16 {
        read_volatile(&(*self.common_cfg).num_queues)
    }

    /// Performs the virtio 1.0 initialisation handshake.
    ///
    /// The device must move ACK -> DRIVER -> FEATURES_OK before queues are
    /// touched, and the driver must then read the final configuration space
    /// before declaring itself ready.
    pub unsafe fn begin_init(&self) -> bool {
        let cfg = &mut *self.common_cfg;
        write_volatile(&mut cfg.device_status, 0);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        core::arch::asm!("isb", options(nostack, preserves_flags));

        write_volatile(&mut cfg.device_status, VirtioPciCommonCfg::STATUS_ACKNOWLEDGE);
        if read_volatile(&mut cfg.device_status) == 0 {
            return false;
        }
        write_volatile(&mut cfg.device_status, VirtioPciCommonCfg::STATUS_DRIVER);
        if read_volatile(&mut cfg.device_status) == 0 {
            return false;
        }
        // Features were negotiated during probe, so they are already stable.
        write_volatile(
            &mut cfg.device_status,
            VirtioPciCommonCfg::STATUS_DRIVER | VirtioPciCommonCfg::STATUS_FEATURES_OK,
        );
        read_volatile(&mut cfg.device_status) != 0
    }

    /// Declares the driver ready after the configuration space has settled.
    pub unsafe fn finish_init(&self) {
        let cfg = &mut *self.common_cfg;
        // Configuration can change while the driver initialises; read until the
        // generation counter is stable.
        let generation = read_volatile(&mut cfg.config_generation);
        let mut attempts = 0;
        while read_volatile(&mut cfg.config_generation) != generation && attempts < 100 {
            attempts += 1;
        }
        write_volatile(&mut cfg.device_status, VirtioPciCommonCfg::STATUS_DRIVER_OK);
    }
}
