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
    for marker in ("pub struct NetDeviceInfo", "pub enum NetworkBackend",
                   "pub struct NetworkDeviceManager", "pub static NETWORK_DEVICE",
                   "pub fn info(&self)", "pub fn set_link_state",
                   "NetworkBackend::Loopback"):
        must(dev, marker, marker)
    must(net, "NETWORK_DEVICE.lock().transmit", "network transmit uses canonical device")
    must(net, "NETWORK_DEVICE.lock()", "network receive uses canonical device")
    must(net, "pub use device::{LoopbackDevice, NetDeviceInfo, NetError, NetworkBackend, NetworkDevice, NetworkDeviceManager, NETWORK_DEVICE};", "network abstraction exports")
    must(pci, "is_network_controller", "PCI network device discovery")
    print("network device abstraction contract: PASS")

if __name__ == "__main__":
    main()
