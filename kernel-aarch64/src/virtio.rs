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
/// The device-specific configuration structure, which is read-only here.
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

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
    /// Device-specific configuration space, when the device publishes a
    /// capability for it. Read-only to this driver.
    pub device_cfg: *const u8,
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
        let mut device_cfg: Option<*const u8> = None;
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
                        VIRTIO_PCI_CAP_DEVICE_CFG => {
                            let bar = ecam.read8(bus, dev, func, ptr as usize + 4);
                            let offset = ecam.read32(bus, dev, func, ptr as usize + 8) as u64;
                            if let Some(base) = Self::map_bar(ecam, bus, dev, func, bar) {
                                device_cfg = Some((base.wrapping_add(offset)) as *const u8);
                            }
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
            device_cfg: device_cfg.unwrap_or(core::ptr::null()),
        })
    }

    /// Reads a little-endian 32-bit field from the device-specific
    /// configuration space.
    ///
    /// Returns `None` when the device published no configuration capability, so
    /// a caller cannot read a field that does not exist.
    ///
    /// # Safety
    /// `index` must be inside the device's configuration structure, and the BAR
    /// holding it must already be mapped.
    pub unsafe fn device_config_u32(&self, index: usize) -> Option<u32> {
        if self.device_cfg.is_null() {
            return None;
        }
        let bytes = core::slice::from_raw_parts(self.device_cfg.add(index * 4), 4);
        Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// True when the device exposes device-specific configuration space.
    pub fn has_device_config(&self) -> bool {
        !self.device_cfg.is_null()
    }

    /// The device's status byte as it currently reads back.
    ///
    /// After a completed handshake this is ACK | DRIVER | FEATURES_OK |
    /// DRIVER_OK, which is the only way to show the device really saw every
    /// step rather than merely that the writes did not fault.
    ///
    /// # Safety
    /// The common configuration structure must be mapped.
    pub unsafe fn device_status(&self) -> u8 {
        read_volatile(&(*self.common_cfg).device_status)
    }

    /// Maps a device BAR so its registers can be reached, and returns its
    /// address.
    ///
    /// The size and the address both come from [`PcieEcam::probe_bar`], the
    /// same probe the bus enumerator uses to decide where a BAR goes, so this
    /// path cannot disagree with the assignment that was written.
    ///
    /// The mapping is idempotent: if the bus enumerator already mapped the
    /// region, the blocks are left alone and this still succeeds. That matters
    /// because the enumerator refuses to let a coarse block shadow DRAM, and
    /// repeating that check here would fail for a region it placed on purpose.
    ///
    /// Returns `None` when the BAR is not implemented, decodes port I/O, or
    /// still holds no address. Driving the device anyway would be guessing at
    /// an address.
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
        let probe = ecam.probe_bar(bus, device, function, bar as usize);
        if !probe.is_memory || probe.size == 0 || probe.size > 1024 * 1024 * 1024 {
            return None;
        }
        let base = probe.address;
        if base == 0 {
            return None;
        }

        // Round outwards to a mapping granularity the MMU can represent.
        const GRANULE: u64 = 0x20_0000;
        let map_base = base & !(GRANULE - 1);
        let map_end = (base + probe.size + GRANULE - 1) & !(GRANULE - 1);

        // Normal memory must never be shadowed, so the platform RAM range is
        // refused. A block that is already mapped is left as it is.
        const PLATFORM_RAM_LO: u64 = 0x4000_0000;
        const PLATFORM_RAM_HI: u64 = 0x8000_0000;
        if !crate::mmu::ArmMmu::map_device_blocks(
            map_base,
            map_end - map_base,
            PLATFORM_RAM_LO,
            PLATFORM_RAM_HI,
        ) {
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
    /// `device_status` is a single byte the device stores verbatim, and the
    /// specification requires the driver to *add* each step to the value
    /// already there. Writing the step on its own would erase the earlier bits,
    /// which the device is entitled to read as the driver having gone away, so
    /// every step here is a read-modify-write.
    ///
    /// Each step is read back and checked, because a device is allowed to drop
    /// back to zero at any point and a driver that only tested for non-zero
    /// would carry on with a device that has forgotten it.
    pub unsafe fn begin_init(&self) -> bool {
        let cfg = &mut *self.common_cfg;
        write_volatile(&mut cfg.device_status, 0);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        core::arch::asm!("isb", options(nostack, preserves_flags));

        let mut expected = VirtioPciCommonCfg::STATUS_ACKNOWLEDGE;
        for step in [
            VirtioPciCommonCfg::STATUS_ACKNOWLEDGE,
            VirtioPciCommonCfg::STATUS_DRIVER,
            // Features were negotiated during probe, so they are already stable.
            VirtioPciCommonCfg::STATUS_FEATURES_OK,
        ] {
            expected |= step;
            write_volatile(&mut cfg.device_status, expected);
            if read_volatile(&mut cfg.device_status) != expected {
                return false;
            }
        }
        true
    }

    /// Declares the driver ready after the configuration space has settled.
    ///
    /// WHY the read-modify-write: `device_status` is cumulative. Writing the
    /// `DRIVER_OK` bit on its own would clear `ACKNOWLEDGE` and `FEATURES_OK`,
    /// and a device that sees its status go backwards treats the driver as gone
    /// and resets.
    pub unsafe fn finish_init(&self) {
        let cfg = &mut *self.common_cfg;
        // Configuration can change while the driver initialises; read until the
        // generation counter is stable.
        let generation = read_volatile(&mut cfg.config_generation);
        let mut attempts = 0;
        while read_volatile(&mut cfg.config_generation) != generation && attempts < 100 {
            attempts += 1;
        }
        let status = read_volatile(&mut cfg.device_status);
        write_volatile(
            &mut cfg.device_status,
            status | VirtioPciCommonCfg::STATUS_DRIVER_OK,
        );
    }
}
