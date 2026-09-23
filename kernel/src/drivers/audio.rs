//! PCI Audio Hardware Controller Driver
//!
//! Enumerates PCI audio controllers (Intel HDA, AC'97, Ensoniq), enables
//! bus mastering for DMA transfers, and configures hardware Base Address Registers (BARs).

use alloc::vec::Vec;
use crate::drivers::pci::{self, PciAddress};

/// Discovered PCI Audio Controller Information
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciAudioDevice {
    pub addr: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub is_hda: bool,
    pub bar0: u32,
    pub interrupt_line: u8,
    pub bus_master_enabled: bool,
}

/// Hardware PCI Audio Controller Manager
pub struct PciAudioController;

impl PciAudioController {
    /// Scans the PCI bus for audio and multimedia controllers.
    pub fn probe_devices() -> Vec<PciAudioDevice> {
        let all_devices = pci::scan_pci_bus();
        let mut audio_devices = Vec::new();

        for dev in &all_devices {
            if dev.is_audio_controller() {
                let bar0 = pci::pci_read_u32(dev.addr, 0x10);
                let is_hda = dev.is_intel_hda();

                // Enable Bus Mastering (Bit 2) and Memory/IO Space (Bits 0 & 1)
                let mut cmd = pci::pci_read_u16(dev.addr, 0x04);
                cmd |= 0x07;
                pci::pci_write_u16(dev.addr, 0x04, cmd);
                let cmd_verified = pci::pci_read_u16(dev.addr, 0x04);
                let bus_master_enabled = (cmd_verified & 0x04) != 0;

                audio_devices.push(PciAudioDevice {
                    addr: dev.addr,
                    vendor_id: dev.vendor_id,
                    device_id: dev.device_id,
                    is_hda,
                    bar0,
                    interrupt_line: dev.interrupt_line,
                    bus_master_enabled,
                });
            }
        }

        audio_devices
    }
}
