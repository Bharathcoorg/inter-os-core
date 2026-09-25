//! Network Interface Controller (NIC) Device Abstraction & Loopback Controller
//!
//! Provides a unified interface for hardware Ethernet adapters and the internal loopback device.

use super::ethernet::MacAddress;

pub const MAX_PACKET_SIZE: usize = 1514;
pub const LOOPBACK_RING_SLOTS: usize = 8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    BufferTooSmall,
    TransmitFailed,
    NoPacketAvailable,
    QueueFull,
    DeviceDown,
    CapabilityDenied,
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
pub struct NetworkDeviceManager {
    backend: LoopbackDevice,
    kind: NetworkBackend,
    mtu: u16,
    link_up: bool,
}

impl NetworkDeviceManager {
    pub const fn new_loopback() -> Self {
        Self {
            backend: LoopbackDevice::new(),
            kind: NetworkBackend::Loopback,
            mtu: 1500,
            link_up: true,
        }
    }

    pub fn info(&self) -> NetDeviceInfo {
        NetDeviceInfo {
            kind: self.kind,
            mac: self.backend.mac(),
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
        self.backend.transmit(frame)
    }

    pub fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> {
        if !self.link_up {
            return Err(NetError::DeviceDown);
        }
        self.backend.receive(out)
    }

    pub fn has_packets(&self) -> bool {
        self.backend.has_packets()
    }
}

impl NetworkDevice for NetworkDeviceManager {
    fn mac(&self) -> MacAddress { self.backend.mac() }
    fn transmit(&mut self, frame: &[u8]) -> Result<(), NetError> { NetworkDeviceManager::transmit(self, frame) }
    fn receive(&mut self, out: &mut [u8]) -> Result<usize, NetError> { NetworkDeviceManager::receive(self, out) }
    fn has_packets(&self) -> bool { self.backend.has_packets() }
}

/// Canonical network-device endpoint consumed by the protocol stack.
pub static NETWORK_DEVICE: spin::Mutex<NetworkDeviceManager> =
    spin::Mutex::new(NetworkDeviceManager::new_loopback());
