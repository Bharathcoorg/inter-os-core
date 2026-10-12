#!/usr/bin/env python3
"""Network device/driver abstraction contracts."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    dev = (ROOT / "kernel/src/net/device.rs").read_text(encoding="utf-8")
    net = (ROOT / "kernel/src/net/mod.rs").read_text(encoding="utf-8")
    pci = (ROOT / "kernel/src/drivers/pci.rs").read_text(encoding="utf-8")
    must(dev, "pub trait NetworkDevice", "network device trait")
    e1000 = (ROOT / "kernel/src/net/e1000.rs").read_text(encoding="utf-8")
    must(e1000, "pub struct E1000Device", "E1000 driver")
    for marker in ("RDBAL", "TDBAL", "pub fn initialize", "impl NetworkDevice for E1000Device",
                   "physical_to_virtual(PhysAddr::new(buffer))", "TX_CMD_RS", "read_link_status",
        "transmit_probe", "receive_probe", "0x10 << 4", "0x40 << 12"):
        must(e1000, marker, marker)
    must(dev, "pub fn probe_transmit", "device manager probe transmit")
    must(dev, "pub fn probe_receive", "device manager probe receive")
    must(dev, "NetworkDeviceBackend::E1000", "E1000 backend selection")
    must(dev, "pub fn attach_e1000", "E1000 device attachment")
    for marker in ("pub struct NetDeviceInfo", "pub enum NetworkBackend",
                   "pub struct NetworkDeviceManager", "pub static NETWORK_DEVICE",
                   "pub fn info(&self)", "pub fn set_link_state",
                   "NetworkBackend::Loopback"):
        must(dev, marker, marker)
    must(net, "NETWORK_DEVICE.lock().transmit", "network transmit uses canonical device")
    must(net, "NETWORK_DEVICE.lock()", "network receive uses canonical device")
    service = (ROOT / "userspace/system-service/src/lib.rs").read_text(encoding="utf-8")
    # RX evidence must come from a real device. Reporting a loopback self-send as
    # an externally received packet is fabricated telemetry, so the contract is
    # that no such claim exists.
    assert "External UDP packet" not in service, \
        "netd must not report loopback traffic as external"
    assert "net_send_socket(socket, [127, 0, 0, 1], 4242" not in service, \
        "netd must not transmit to its own loopback socket"
    for symbol in ("LoopbackDevice", "NetDeviceInfo", "NetError", "NetworkBackend",
                 "NetworkDevice", "NetworkDeviceManager", "NETWORK_DEVICE"):
        must(net, symbol, f"network abstraction export {symbol}")
    # Binding a probed NIC is gated by ENABLE_E1000_BINDING, kept as a
    # deliberate regression switch after the descriptor DMA mapping was proven
    # coherent and the guest was verified to boot with the controller enabled.
    must(net, "ENABLE_E1000_BINDING", "E1000 binding gate export")
    # WHY the pin now expects true: the E1000 NIC DMAs into descriptor/buffer
    # pages, and the earlier page fault came from a physical/virtual mismatch.
    # That is now verified correct: allocate_dma_frames returns PHYSICAL frames,
    # e1000.rs writes those physical addresses into RDBAL/TDBAL, and the CPU
    # reaches the SAME memory through the bootloader direct map
    # (physical_to_virtual). Physical and virtual name identical pages, so the
    # ring is coherent. End-to-end evidence: attach_e1000 emits a real ARP
    # request for 10.0.2.2 and poll_network_interface logs QEMU's real reply, so
    # a packet demonstrably leaves the guest and comes back. The gate is left in
    # place so the datapath can still be disabled without touching the driver.
    must(dev, "pub const ENABLE_E1000_BINDING: bool = true", "E1000 binding enabled after DMA mapping verification")
    must(dev, "if !ENABLE_E1000_BINDING", "attach refuses while binding is disabled (regression switch retained)")
    # The gate must be evaluated before the controller is enabled, since
    # initialize() starts DMA.
    must(pci, "if !crate::net::ENABLE_E1000_BINDING", "PCI attach gated before enable")
    must(pci, "nic.initialize()", "E1000 initialize call retained")
    gate_at = pci.index("if !crate::net::ENABLE_E1000_BINDING")
    init_at = pci.index("nic.initialize()")
    assert gate_at < init_at, "E1000 binding gate must precede controller initialization"
    must(pci, "is_network_controller", "PCI network device discovery")
    must(pci, "pub fn memory_bar", "PCI BAR discovery")
    must(pci, "pub fn enable_memory_and_bus_master", "PCI DMA enablement")
    must(pci, "pub fn find_e1000", "E1000 PCI discovery")
    must(pci, "pub fn initialize_e1000_network", "E1000 hardware attachment")
    must(pci, "map_mmio_region", "MMIO mapping")
    memory = (ROOT / "kernel/src/memory.rs").read_text(encoding="utf-8")
    must(memory, "pub fn allocate_dma_frames", "contiguous DMA frame allocation")
    must(memory, "pub fn map_mmio_region", "kernel MMIO mapping")
    print("network device abstraction contract: PASS")

if __name__ == "__main__":
    main()
