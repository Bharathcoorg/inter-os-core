#!/usr/bin/env python3
"""M31 network device/driver abstraction contracts."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    dev = (ROOT / "kernel/src/net/device.rs").read_text()
    net = (ROOT / "kernel/src/net/mod.rs").read_text()
    pci = (ROOT / "kernel/src/drivers/pci.rs").read_text()
    must(dev, "pub trait NetworkDevice", "network device trait")
    e1000 = (ROOT / "kernel/src/net/e1000.rs").read_text()
    must(e1000, "pub struct E1000Device", "E1000 driver")
    for marker in ("RDBAL", "TDBAL", "pub fn initialize", "impl NetworkDevice for E1000Device"):
        must(e1000, marker, marker)
    must(dev, "NetworkDeviceBackend::E1000", "E1000 backend selection")
    must(dev, "pub fn attach_e1000", "E1000 device attachment")
    for marker in ("pub struct NetDeviceInfo", "pub enum NetworkBackend",
                   "pub struct NetworkDeviceManager", "pub static NETWORK_DEVICE",
                   "pub fn info(&self)", "pub fn set_link_state",
                   "NetworkBackend::Loopback"):
        must(dev, marker, marker)
    must(net, "NETWORK_DEVICE.lock().transmit", "network transmit uses canonical device")
    must(net, "NETWORK_DEVICE.lock()", "network receive uses canonical device")
    must(net, "pub use device::{LoopbackDevice, NetDeviceInfo, NetError, NetworkBackend, NetworkDevice, NetworkDeviceManager, NETWORK_DEVICE};", "network abstraction exports")
    must(pci, "is_network_controller", "PCI network device discovery")
    must(pci, "pub fn memory_bar", "PCI BAR discovery")
    must(pci, "pub fn enable_memory_and_bus_master", "PCI DMA enablement")
    must(pci, "pub fn find_e1000", "E1000 PCI discovery")
    must(pci, "pub fn initialize_e1000_network", "E1000 hardware attachment")
    must(pci, "map_mmio_region", "MMIO mapping")
    memory = (ROOT / "kernel/src/memory.rs").read_text()
    must(memory, "pub fn allocate_dma_frames", "contiguous DMA frame allocation")
    must(memory, "pub fn map_mmio_region", "kernel MMIO mapping")
    print("network device abstraction contract: PASS")

if __name__ == "__main__":
    main()
