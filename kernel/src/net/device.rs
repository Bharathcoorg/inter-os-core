//! Network Interface Controller (NIC) Device Abstraction & Loopback Controller
//!
//! Provides a unified interface for hardware Ethernet adapters and the internal loopback device.

use super::ethernet::MacAddress;
use super::e1000::E1000Device;

pub const MAX_PACKET_SIZE: usize = 1514;
pub const LOOPBACK_RING_SLOTS: usize = 8;

/// Whether a probed E1000 may be bound as the active network device.
///
/// WHY the gate existed: the E1000 station address is now read correctly from
/// the EEPROM, so probing a real NIC succeeds where it previously always
/// failed. But once bound, the controller is enabled and starts DMA into
/// descriptor/buffer memory owned by the mapping layer, and that path used to
/// take the guest down with a kernel page fault during boot.
///
/// WHY it is open now: the descriptor mapping was *verified*, not assumed. The
/// NIC must be handed *physical* base addresses while the CPU reaches the same
/// memory through a *virtual* mapping; if those disagree the device DMAs to one
/// page and the CPU faults reading another. `allocate_dma_frames` (memory.rs)
/// returns physical `PhysFrame`s; e1000.rs writes those physical addresses into
/// RDBAL/TDBAL and keeps the CPU-side pointers on the bootloader direct map via
/// `physical_to_virtual`. Physical and virtual therefore name the same pages,
/// so the descriptor ring is coherent and the guest boots to a real ARP
/// exchange (a request leaves the NIC in `emit_gateway_arp_request`, and the
/// QEMU user-mode gateway's real reply is logged from `poll_network_interface`).
///
/// Kept as a regression switch: setting this back to false makes binding fail
/// closed to the loopback backend without changing the driver, and the boot
/// banner reports which backend is active either way.
pub const ENABLE_E1000_BINDING: bool = true;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    BufferTooSmall,
    TransmitFailed,
    NoPacketAvailable,
    QueueFull,
    DeviceDown,
    CapabilityDenied,
    InvalidPort,
    InvalidPacket,
    PortInUse,
    NotReady,
}

/// Unified Network Device Driver Interface
pub trait NetworkDevice {
    fn mac(&self) -> MacAddress;
    fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError>;
    fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError>;
    fn has_packets(&self) -> bool;
}

/// Fixed-capacity loopback packet buffer
#[derive(Copy, Clone)]
struct PacketSlot {
    data: [u8; MAX_PACKET_SIZE],
    len: usize,
    valid: bool,
}

/// Zero-allocation In-Kernel Loopback Network Device
pub struct LoopbackDevice {
    mac: MacAddress,
    slots: [PacketSlot; LOOPBACK_RING_SLOTS],
    read_idx: usize,
    write_idx: usize,
    count: usize,
    pub packets_transmitted: u64,
    pub packets_received: u64,
}

impl LoopbackDevice {
    pub const fn new() -> Self {
        Self {
            mac: MacAddress([0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
            slots: [PacketSlot {
                data: [0u8; MAX_PACKET_SIZE],
                len: 0,
                valid: false,
            }; LOOPBACK_RING_SLOTS],
            read_idx: 0,
            write_idx: 0,
            count: 0,
            packets_transmitted: 0,
            packets_received: 0,
        }
    }
}

impl NetworkDevice for LoopbackDevice {
    fn mac(&self) -> MacAddress {
        self.mac
    }

    fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if frame.len() > MAX_PACKET_SIZE {
            return Err(NetError::BufferTooSmall);
        }
        if self.count >= LOOPBACK_RING_SLOTS {
            return Err(NetError::QueueFull);
        }

        let slot = &mut self.slots[self.write_idx];
        slot.data[0..frame.len()].copy_from_slice(frame);
        slot.len = frame.len();
        slot.valid = true;

        self.write_idx = (self.write_idx + 1) % LOOPBACK_RING_SLOTS;
        self.count += 1;
        self.packets_transmitted += 1;

        Ok(())
    }

    fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        if self.count == 0 {
            return Err(NetError::NoPacketAvailable);
        }

        let slot = &mut self.slots[self.read_idx];
        if !slot.valid {
            return Err(NetError::NoPacketAvailable);
        }

        let len = slot.len;
        if out.len() < len {
            return Err(NetError::BufferTooSmall);
        }

        out[0..len].copy_from_slice(&slot.data[0..len]);
        slot.valid = false;

        self.read_idx = (self.read_idx + 1) % LOOPBACK_RING_SLOTS;
        self.count -= 1;
        self.packets_received += 1;

        Ok(len)
    }

    fn has_packets(&self) -> bool {
        self.count > 0
    }
}


/// Stable identity and capabilities advertised by a network device.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NetDeviceInfo {
    pub kind: NetworkBackend,
    pub mac: MacAddress,
    pub mtu: u16,
    pub link_up: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NetworkBackend {
    Loopback,
    EthernetPci,
    Wifi,
    Cellular,
}

/// Kernel-owned network-device manager. The protocol stack talks only to this
/// boundary, allowing physical NIC drivers to replace loopback without changing
/// Ethernet/IP/socket code.
pub enum NetworkDeviceBackend {
    Loopback(LoopbackDevice),
    E1000(E1000Device),
}

pub struct NetworkDeviceManager {
    backend: NetworkDeviceBackend,
    kind: NetworkBackend,
    mtu: u16,
    link_up: bool,
}

impl NetworkDeviceManager {
    pub const fn new_loopback() -> Self {
        Self {
            backend: NetworkDeviceBackend::Loopback(LoopbackDevice::new()),
            kind: NetworkBackend::Loopback,
            mtu: 1500,
            link_up: true,
        }
    }

    pub fn info(&self) -> NetDeviceInfo {
        NetDeviceInfo {
            kind: self.kind,
            mac: match &self.backend { NetworkDeviceBackend::Loopback(d) => d.mac(), NetworkDeviceBackend::E1000(d) => d.mac() },
            mtu: self.mtu,
            link_up: self.link_up,
        }
    }

    pub fn set_link_state(&mut self, up: bool) {
        self.link_up = up;
    }

    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if !self.link_up {
            return Err(NetError::DeviceDown);
        }
        if frame.len() > self.mtu as usize + 14 {
            return Err(NetError::BufferTooSmall);
        }
        match &mut self.backend { NetworkDeviceBackend::Loopback(d) => d.transmit(frame), NetworkDeviceBackend::E1000(d) => d.transmit(frame) }
    }

    pub fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        if !self.link_up {
            return Err(NetError::DeviceDown);
        }
        match &mut self.backend { NetworkDeviceBackend::Loopback(d) => d.receive(out), NetworkDeviceBackend::E1000(d) => d.receive(out) }
    }

    pub fn probe_transmit(&mut self, frame: &[u8]) -> Result<(), NetError> {
        match &mut self.backend {
            NetworkDeviceBackend::Loopback(d) => d.transmit(frame),
            NetworkDeviceBackend::E1000(d) => d.transmit_probe(frame),
        }
    }

    pub fn probe_receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        match &mut self.backend {
            NetworkDeviceBackend::Loopback(d) => d.receive(out),
            NetworkDeviceBackend::E1000(d) => d.receive_probe(out),
        }
    }

    pub fn has_packets(&self) -> bool {
        match &self.backend { NetworkDeviceBackend::Loopback(d) => d.has_packets(), NetworkDeviceBackend::E1000(d) => d.has_packets() }
    }

/// Replaces the loopback backend with an initialized Ethernet controller.
///
/// Refuses to bind while [`ENABLE_E1000_BINDING`] is false, leaving the
/// loopback backend in place. The constant is the single regression switch for
/// the E1000 DMA datapath that is now bound by default.
pub fn attach_e1000(&mut self, device: E1000Device) -> bool {
    if !ENABLE_E1000_BINDING {
        return false;
    }
    self.backend = NetworkDeviceBackend::E1000(device);
    self.kind = NetworkBackend::EthernetPci;
    self.mtu = 1500;
    self.link_up = true;
    // The controller is now the live backend and its descriptor ring is being
    // serviced. Emit one real ARP request for the default gateway so the first
    // frame this NIC ever sends actually leaves the guest. This drives the
    // manager directly (the NETWORK_DEVICE lock is already held for this call),
    // so there is no re-entrant lock; the matching reply is observed and logged
    // on the next network poll.
    crate::net::emit_gateway_arp_request(self);
    true
}
}

impl NetworkDevice for NetworkDeviceManager {
    fn mac(&self) -> MacAddress { self.info().mac }
    fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError> { NetworkDeviceManager::transmit(self, frame) }
    fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> { NetworkDeviceManager::receive(self, out) }
    fn has_packets(&self) -> bool {
        NetworkDeviceManager::has_packets(self)
    }
}

pub static NETWORK_DEVICE: spin::Mutex<NetworkDeviceManager> =
    spin::Mutex::new(NetworkDeviceManager::new_loopback());
