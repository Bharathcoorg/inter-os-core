//! Zero-Trust Capability-Governed Network Sockets
//!
//! Sockets are protected kernel objects. Processes must possess explicit
//! `NetworkSocket`, `NetworkSend`, and `NetworkReceive` capabilities to interact with them.

use super::ipv4::Ipv4Address;
use super::device::NetError;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

pub const MAX_SOCKETS: usize = 8;
pub const SOCKET_RX_SLOTS: usize = 4;
pub const SOCKET_RX_BUF_SIZE: usize = 512;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SocketId(pub u32);

#[derive(Copy, Clone)]
struct RxDatagram {
    data: [u8; SOCKET_RX_BUF_SIZE],
    len: usize,
    src_ip: Ipv4Address,
    src_port: u16,
    valid: bool,
}

/// Capability-Guarded UDP Socket Handle
pub struct UdpSocket {
    pub id: SocketId,
    pub local_port: u16,
    pub owner_task_id: u32,
    rx_slots: [RxDatagram; SOCKET_RX_SLOTS],
    rx_read: usize,
    rx_write: usize,
    rx_count: usize,
    pub is_bound: bool,
}

impl UdpSocket {
    pub const fn new(id: SocketId, owner_task_id: u32) -> Self {
        Self {
            id,
            local_port: 0,
            owner_task_id,
            rx_slots: [RxDatagram {
                data: [0u8; SOCKET_RX_BUF_SIZE],
                len: 0,
                src_ip: Ipv4Address::UNSPECIFIED,
                src_port: 0,
                valid: false,
            }; SOCKET_RX_SLOTS],
            rx_read: 0,
            rx_write: 0,
            rx_count: 0,
            is_bound: false,
        }
    }

    /// Enqueues an incoming received datagram into the socket's internal mailbox.
    pub fn enqueue_rx(&mut self, src_ip: Ipv4Address, src_port: u16, payload: &[u8]) -> Result<(), NetError> {
        if self.rx_count >= SOCKET_RX_SLOTS {
            return Err(NetError::QueueFull);
        }
        let copy_len = if payload.len() > SOCKET_RX_BUF_SIZE {
            SOCKET_RX_BUF_SIZE
        } else {
            payload.len()
        };

        let slot = &mut self.rx_slots[self.rx_write];
        slot.data[0..copy_len].copy_from_slice(&payload[0..copy_len]);
        slot.len = copy_len;
        slot.src_ip = src_ip;
        slot.src_port = src_port;
        slot.valid = true;

        self.rx_write = (self.rx_write + 1) % SOCKET_RX_SLOTS;
        self.rx_count += 1;
        Ok(())
    }

    /// Receives a datagram from the socket queue, verifying caller's capability.
    pub fn recv_from(
        &mut self,
        out: &mut [u8],
        caps: &CapabilityTable,
    ) -> Result<(usize, Ipv4Address, u16), NetError> {
        // Enforce Zero-Trust Capability Invariant
        if !caps.has_capability(CapabilityType::NetworkReceive, Rights::READ) {
            return Err(NetError::CapabilityDenied);
        }

        if self.rx_count == 0 {
            return Err(NetError::NoPacketAvailable);
        }

        let slot = &mut self.rx_slots[self.rx_read];
        if !slot.valid {
            return Err(NetError::NoPacketAvailable);
        }

        let copy_len = if out.len() < slot.len { out.len() } else { slot.len };
        out[0..copy_len].copy_from_slice(&slot.data[0..copy_len]);
        let src_ip = slot.src_ip;
        let src_port = slot.src_port;

        slot.valid = false;
        self.rx_read = (self.rx_read + 1) % SOCKET_RX_SLOTS;
        self.rx_count -= 1;

        Ok((copy_len, src_ip, src_port))
    }
}

/// Global In-Kernel Socket Registry
pub struct SocketTable {
    sockets: [Option<UdpSocket>; MAX_SOCKETS],
    next_id: u32,
}

impl SocketTable {
    pub const fn new() -> Self {
        Self {
            sockets: [None, None, None, None, None, None, None, None],
            next_id: 1,
        }
    }

    /// Creates and binds a new UDP socket to a local port with capability check.
    pub fn create_socket(
        &mut self,
        owner_task_id: u32,
        port: u16,
        caps: &CapabilityTable,
    ) -> Result<SocketId, NetError> {
        // Enforce NetworkSocket creation capability
        if !caps.has_capability(CapabilityType::NetworkSocket, Rights::WRITE) {
            return Err(NetError::CapabilityDenied);
        }

        for slot in self.sockets.iter_mut() {
            if slot.is_none() {
                let id = SocketId(self.next_id);
                self.next_id += 1;
                let mut sock = UdpSocket::new(id, owner_task_id);
                sock.local_port = port;
                sock.is_bound = true;
                *slot = Some(sock);
                return Ok(id);
            }
        }

        Err(NetError::QueueFull)
    }

    /// Delivers an incoming UDP datagram to any bound socket matching the destination port.
    pub fn deliver_udp(&mut self, dst_port: u16, src_ip: Ipv4Address, src_port: u16, payload: &[u8]) -> bool {
        let mut delivered = false;
        for sock_opt in self.sockets.iter_mut() {
            if let Some(ref mut sock) = sock_opt {
                if sock.is_bound && sock.local_port == dst_port {
                    if sock.enqueue_rx(src_ip, src_port, payload).is_ok() {
                        delivered = true;
                    }
                }
            }
        }
        delivered
    }

    /// Retrieves a mutable reference to a socket by ID.
    pub fn get_mut(&mut self, id: SocketId) -> Option<&mut UdpSocket> {
        for sock_opt in self.sockets.iter_mut() {
            if let Some(ref mut sock) = sock_opt {
                if sock.id == id {
                    return Some(sock);
                }
            }
        }
        None
    }
}
