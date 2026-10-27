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
pub const TCP_RX_BUF_SIZE: usize = 16384;
pub const TCP_TX_BUF_SIZE: usize = crate::net::tcp::TCP_MAX_SEGMENT;
pub const TCP_RETRANSMIT_TICKS: u64 = 500;
pub const TCP_MAX_RETRIES: u8 = 3;

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
    pub rx_count: usize,
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

        if !caps.authorize_capability(CapabilityType::NetworkReceive, Rights::READ) {
            return Err(NetError::CapabilityDenied);
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

        let process = crate::task::process::process_for_task(crate::task::TaskId(owner_task_id as u64));
        if let Some(process) = process {
            crate::task::process::reserve_network_socket(process)
                .map_err(|_| NetError::QueueFull)?;
        }

        if port == 0 { 
            if let Some(process) = process { crate::task::process::release_network_socket(process); }
            return Err(NetError::InvalidPort);
        }
        if self.sockets.iter().flatten().any(|sock| sock.is_bound && sock.local_port == port) {
            if let Some(process) = process { crate::task::process::release_network_socket(process); }
            return Err(NetError::PortInUse);
        }
        for slot in self.sockets.iter_mut() {
            if slot.is_none() {
                if !caps.authorize_capability(CapabilityType::NetworkSocket, Rights::WRITE) {
                    if let Some(process) = process { crate::task::process::release_network_socket(process); }
                    return Err(NetError::CapabilityDenied);
                }
                let id = SocketId(self.next_id);
                self.next_id += 1;
                let mut sock = UdpSocket::new(id, owner_task_id);
                sock.local_port = port;
                sock.is_bound = true;
                *slot = Some(sock);
                return Ok(id);
            }
        }

        if let Some(process) = process { crate::task::process::release_network_socket(process); }
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
    /// Releases every socket owned by a terminated task.
    pub fn release_owner(&mut self, owner_task_id: u32) -> usize {
        let mut released = 0;
        for slot in self.sockets.iter_mut() {
            if slot.as_ref().map(|sock| sock.owner_task_id == owner_task_id).unwrap_or(false) {
                *slot = None;
                released += 1;
            }
        }
        if released > 0 {
            if let Some(process) = crate::task::process::process_for_task(crate::task::TaskId(owner_task_id as u64)) {
                for _ in 0..released { crate::task::process::release_network_socket(process); }
            }
        }
        released
    }

    /// Closes a socket only when its owner matches the caller.
    pub fn close_owned(&mut self, id: SocketId, owner_task_id: u32) -> Result<(), NetError> {
        for slot in self.sockets.iter_mut() {
            if slot.as_ref().map(|sock| sock.id == id).unwrap_or(false) {
                if slot.as_ref().unwrap().owner_task_id != owner_task_id {
                    return Err(NetError::CapabilityDenied);
                }
                *slot = None;
                if let Some(process) = crate::task::process::process_for_task(crate::task::TaskId(owner_task_id as u64)) {
                    crate::task::process::release_network_socket(process);
                }
                return Ok(());
            }
        }
        Err(NetError::NoPacketAvailable)
    }

    pub fn get(&self, id: SocketId) -> Option<&UdpSocket> {
        for sock_opt in self.sockets.iter() {
            if let Some(ref sock) = sock_opt {
                if sock.id == id {
                    return Some(sock);
                }
            }
        }
        None
    }

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


#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TcpConnection {
    pub id: u32,
    pub owner_task_id: u32,
    pub local_port: u16,
    pub remote_ip: Ipv4Address,
    pub remote_port: u16,
    pub state: crate::net::TcpState,
    pub send_seq: u32,
    pub snd_una: u32,
    pub snd_wnd: u32,
    pub cwnd: u32,
    pub ssthresh: u32,
    pub dup_acks: u8,
    pub recv_seq: u32,
    pub rcv_wnd: u32,
    pub rx: [u8; TCP_RX_BUF_SIZE],
    pub rx_len: usize,
    pub tx: [u8; TCP_TX_BUF_SIZE],
    pub tx_len: usize,
    pub tx_seq: u32,
    pub tx_deadline: u64,
    pub tx_retries: u8,
    pub tx_pending: bool,
    pub fin_pending: bool,
    pub fin_seq: u32,
    pub fin_deadline: u64,
    pub fin_retries: u8,
    pub syn_pending: bool,
    pub syn_seq: u32,
    pub syn_deadline: u64,
    pub syn_retries: u8,
    pub time_wait_until: u64,
}

pub struct TcpConnectionTable {
    entries: [Option<TcpConnection>; 8],
    next_id: u32,
}

impl TcpConnectionTable {
    pub const fn new() -> Self {
        Self { entries: [None; 8], next_id: 1 }
    }

    pub fn open(
        &mut self,
        owner_task_id: u32,
        local_port: u16,
        remote_ip: Ipv4Address,
        remote_port: u16,
        initial_seq: u32,
    ) -> Result<u32, NetError> {
        if local_port == 0 || remote_port == 0 {
            return Err(NetError::InvalidPort);
        }
        let process = crate::task::process::process_for_task(crate::task::TaskId(owner_task_id as u64));
        if let Some(process) = process {
            crate::task::process::reserve_network_socket(process)
                .map_err(|_| NetError::QueueFull)?;
        }
        if self.entries.iter().flatten().any(|conn| {
            conn.local_port == local_port
                && conn.remote_ip == remote_ip
                && conn.remote_port == remote_port
                && conn.state != crate::net::TcpState::Closed
        }) {
            if let Some(process) = process { crate::task::process::release_network_socket(process); }
            return Err(NetError::PortInUse);
        }

        for slot in self.entries.iter_mut() {
            if slot.is_none() {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                *slot = Some(TcpConnection {
                    id,
                    owner_task_id,
                    local_port,
                    remote_ip,
                    remote_port,
                    state: crate::net::TcpState::SynSent,
                    send_seq: initial_seq,
                    snd_una: initial_seq,
                    snd_wnd: 65535,
                    cwnd: crate::net::tcp::TCP_MAX_SEGMENT as u32,
                    ssthresh: 65535,
                    dup_acks: 0,
                    recv_seq: 0,
                    rcv_wnd: TCP_RX_BUF_SIZE as u32,
                    rx: [0u8; TCP_RX_BUF_SIZE],
                    rx_len: 0,
                    tx: [0u8; TCP_TX_BUF_SIZE],
                    tx_len: 0,
                    tx_seq: initial_seq,
                    tx_deadline: 0,
                    tx_retries: 0,
                    tx_pending: false,
                    fin_pending: false,
                    fin_seq: initial_seq,
                    fin_deadline: 0,
                    fin_retries: 0,
                    syn_pending: true,
                    syn_seq: initial_seq,
                    // A fresh connection's first retransmission is due one
                    // timeout from now. Using the bare timeout value made the
                    // SYN immediately overdue once system uptime passed it,
                    // so the poll path retransmitted on its next pass.
                    syn_deadline: crate::interrupts::get_system_ticks()
                        .saturating_add(TCP_RETRANSMIT_TICKS),
                    syn_retries: 0,
                    time_wait_until: 0,
                });
                return Ok(id);
            }
        }
        if let Some(process) = process { crate::task::process::release_network_socket(process); }
        Err(NetError::QueueFull)
    }

    pub fn entries(&self) -> &[Option<TcpConnection>; 8] { &self.entries }
    pub fn entries_mut(&mut self) -> core::slice::IterMut<'_, Option<TcpConnection>> { self.entries.iter_mut() }

    pub fn get(&self, id: u32) -> Option<&TcpConnection> {
        self.entries.iter().flatten().find(|c| c.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut TcpConnection> {
        self.entries.iter_mut().flatten().find(|c| c.id == id)
    }

    pub fn enqueue_rx(&mut self, id: u32, owner_task_id: u32, payload: &[u8]) -> Result<usize, NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if conn.owner_task_id != owner_task_id { return Err(NetError::CapabilityDenied); }
        let room = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len);
        let n = payload.len().min(room);
        conn.rx[conn.rx_len..conn.rx_len+n].copy_from_slice(&payload[..n]);
        conn.rx_len += n;
        Ok(n)
    }

    /// Accepts only the next in-order TCP sequence range. Duplicate data is
    /// acknowledged by the caller without being appended a second time.
    pub fn accept_rx_segment(&mut self, id: u32, seq: u32, payload: &[u8], fin: bool) -> Result<usize, NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if conn.state != crate::net::TcpState::Established { return Err(NetError::NoPacketAvailable); }
        let end = seq.wrapping_add(payload.len() as u32).wrapping_add(if fin { 1 } else { 0 });
        if !crate::net::tcp::seq_gt(end, conn.recv_seq) {
            return Ok(0);
        }
        let offset = if crate::net::tcp::seq_lt(seq, conn.recv_seq) {
            conn.recv_seq.wrapping_sub(seq) as usize
        } else {
            0
        };
        // Sequence check: seq != conn.recv_seq enforcement (offset-aware for TCP overlaps)
        if seq.wrapping_add(offset as u32) != conn.recv_seq {
            return Err(NetError::NoPacketAvailable);
        }
        let new_payload = if offset >= payload.len() {
            &[]
        } else {
            &payload[offset..]
        };
        let room = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len);
        let n = new_payload.len().min(room);
        if n != new_payload.len() && !new_payload.is_empty() { return Err(NetError::QueueFull); }
        conn.rx[conn.rx_len..conn.rx_len + n].copy_from_slice(new_payload);
        conn.rx_len += n;
        conn.recv_seq = conn.recv_seq.wrapping_add(n as u32);
        conn.rcv_wnd = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len) as u32;
        if fin && n == new_payload.len() {
            conn.recv_seq = conn.recv_seq.wrapping_add(1);
            conn.state = crate::net::TcpState::CloseWait;
        }
        Ok(n)
    }

    pub fn arm_retransmit(&mut self, id: u32, seq: u32, payload: &[u8], now: u64) -> Result<(), NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if payload.len() > TCP_TX_BUF_SIZE { return Err(NetError::BufferTooSmall); }
        conn.tx[..payload.len()].copy_from_slice(payload);
        conn.tx_len = payload.len();
        conn.tx_seq = seq;
        conn.tx_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS);
        conn.tx_retries = 0;
        conn.tx_pending = !payload.is_empty();
        Ok(())
    }

    pub fn acknowledge_tx(&mut self, id: u32, ack: u32) -> bool {
        let conn = match self.get_mut(id) { Some(c) => c, None => return false };
        let end = conn.send_seq;
        if crate::net::tcp::seq_lt(ack, conn.snd_una) || crate::net::tcp::seq_gt(ack, end) { return false; }
        if ack == conn.snd_una {
            if conn.tx_pending { conn.dup_acks = conn.dup_acks.saturating_add(1); if conn.dup_acks >= 3 { conn.ssthresh = (conn.cwnd / 2).max(crate::net::tcp::TCP_MAX_SEGMENT as u32); conn.cwnd = conn.ssthresh; } }
            return false;
        }
        conn.snd_una = ack;
        conn.dup_acks = 0;
        if conn.cwnd < conn.ssthresh { conn.cwnd = conn.cwnd.saturating_add(crate::net::tcp::TCP_MAX_SEGMENT as u32).min(65535); }
        else { let increment = ((crate::net::tcp::TCP_MAX_SEGMENT as u32).saturating_mul(crate::net::tcp::TCP_MAX_SEGMENT as u32) / conn.cwnd.max(1)).max(1); conn.cwnd = conn.cwnd.saturating_add(increment).min(65535); }
        if conn.tx_pending {
            let end = conn.tx_seq.wrapping_add(conn.tx_len as u32);
            if crate::net::tcp::seq_ge(ack, end) {
                conn.tx_pending = false; conn.tx_len = 0; conn.tx_retries = 0;
            } else if crate::net::tcp::seq_gt(ack, conn.tx_seq) {
                let acknowledged = ack.wrapping_sub(conn.tx_seq) as usize;
                if acknowledged < conn.tx_len {
                    conn.tx.copy_within(acknowledged..conn.tx_len, 0);
                    conn.tx_len -= acknowledged;
                    conn.tx_seq = ack;
                    conn.tx_retries = 0;
                    conn.tx_deadline = crate::interrupts::get_system_ticks().saturating_add(TCP_RETRANSMIT_TICKS);
                }
            }
        }
        true
    }

    pub fn update_peer_window(&mut self, id: u32, advertised: u16) -> Result<(), NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        conn.snd_wnd = advertised as u32;
        Ok(())
    }

    pub fn send_window(&self, id: u32) -> Option<u32> {
        let conn = self.get(id)?;
        let in_flight = conn.send_seq.wrapping_sub(conn.snd_una);
        Some(conn.cwnd.min(conn.snd_wnd).saturating_sub(in_flight))
    }

    pub fn due_retransmit(&mut self, now: u64) -> Option<(u32, Ipv4Address, u16, u16, u32, u32, usize)> {
        for slot in self.entries.iter_mut() {
            if let Some(conn) = slot.as_mut() {
                if conn.tx_pending && now >= conn.tx_deadline {
                    if conn.tx_retries >= TCP_MAX_RETRIES {
                        // Exhaustion is a terminal transport failure. Do not
                        // silently discard the payload while leaving the
                        // connection Established and usable.
                        crate::serial_println!(
                            "[TCP TX TIMEOUT] Connection {} to {}.{}.{}.{}:{} closed: max retries ({}) exhausted",
                            conn.id, conn.remote_ip.0[0], conn.remote_ip.0[1], conn.remote_ip.0[2], conn.remote_ip.0[3], conn.remote_port, conn.tx_retries
                        );
                        conn.tx_pending = false;
                        conn.tx_len = 0;
                        conn.tx_retries = 0;
                        conn.tx_deadline = 0;
                        conn.state = crate::net::TcpState::Closed;
                        continue;
                    }
                    conn.tx_retries += 1;
                    conn.tx_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS << conn.tx_retries);
                    crate::serial_println!(
                        "[TCP TX RETRY] Connection {} retry {}/{} to {}.{}.{}.{}:{}",
                        conn.id, conn.tx_retries, TCP_MAX_RETRIES,
                        conn.remote_ip.0[0], conn.remote_ip.0[1], conn.remote_ip.0[2], conn.remote_ip.0[3], conn.remote_port
                    );
                    return Some((conn.id, conn.remote_ip, conn.local_port, conn.remote_port, conn.tx_seq, conn.recv_seq, conn.tx_len));
                }
            }
        }
        None
    }

    pub fn arm_syn(&mut self, id: u32, now: u64) -> Result<(), NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if conn.state != crate::net::TcpState::SynSent { return Err(NetError::NotReady); }
        conn.syn_pending = true;
        conn.syn_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS);
        conn.syn_retries = 0;
        Ok(())
    }

    pub fn acknowledge_syn(&mut self, id: u32) -> bool {
        if let Some(conn) = self.get_mut(id) {
            conn.syn_pending = false;
            conn.syn_retries = 0;
            return true;
        }
        false
    }

    pub fn due_syn_retransmit(&mut self, now: u64) -> Option<(u32, Ipv4Address, u16, u16, u32, u16)> {
        for slot in self.entries.iter_mut() {
            if let Some(conn) = slot.as_mut() {
                if conn.syn_pending && conn.state == crate::net::TcpState::SynSent && now >= conn.syn_deadline {
                    if conn.syn_retries >= TCP_MAX_RETRIES {
                        crate::serial_println!(
                            "[TCP SYN TIMEOUT] Connection {} to {}.{}.{}.{}:{} closed: SYN retries ({}) exhausted",
                            conn.id, conn.remote_ip.0[0], conn.remote_ip.0[1], conn.remote_ip.0[2], conn.remote_ip.0[3], conn.remote_port, conn.syn_retries
                        );
                        conn.syn_pending = false;
                        conn.state = crate::net::TcpState::Closed;
                        continue;
                    }
                    conn.syn_retries += 1;
                    conn.syn_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS << conn.syn_retries);
                    crate::serial_println!(
                        "[TCP SYN RETRY] Connection {} retry {}/{} to {}.{}.{}.{}:{}",
                        conn.id, conn.syn_retries, TCP_MAX_RETRIES,
                        conn.remote_ip.0[0], conn.remote_ip.0[1], conn.remote_ip.0[2], conn.remote_ip.0[3], conn.remote_port
                    );
                    return Some((conn.id, conn.remote_ip, conn.local_port, conn.remote_port, conn.syn_seq, conn.rcv_wnd.min(u16::MAX as u32) as u16));
                }
            }
        }
        None
    }

    pub fn arm_fin(&mut self, id: u32, now: u64) -> Result<u32, NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        let next_state = match conn.state {
            crate::net::TcpState::Established => crate::net::TcpState::FinWait,
            crate::net::TcpState::CloseWait => crate::net::TcpState::LastAck,
            _ => return Err(NetError::NoPacketAvailable),
        };
        conn.fin_seq = conn.send_seq;
        conn.fin_pending = true;
        conn.fin_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS);
        conn.fin_retries = 0;
        conn.send_seq = conn.send_seq.wrapping_add(1);
        conn.state = next_state;
        Ok(conn.fin_seq)
    }

    /// Rolls back a locally armed FIN when the initial wire transmission fails.
    /// This prevents the connection from becoming permanently half-closed when
    /// the transport layer rejected the first FIN before it left the kernel.
    pub fn rollback_fin(&mut self, id: u32) -> Result<(), NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if !conn.fin_pending { return Err(NetError::NotReady); }
        conn.send_seq = conn.send_seq.wrapping_sub(1);
        conn.fin_pending = false;
        conn.fin_retries = 0;
        conn.fin_deadline = 0;
        conn.state = match conn.state {
            crate::net::TcpState::FinWait => crate::net::TcpState::Established,
            crate::net::TcpState::LastAck => crate::net::TcpState::CloseWait,
            state => state,
        };
        Ok(())
    }

    pub fn acknowledge_fin(&mut self, id: u32, ack: u32) -> bool {
        let conn = match self.get_mut(id) { Some(c) => c, None => return false };
        if conn.fin_pending && crate::net::tcp::seq_ge(ack, conn.fin_seq.wrapping_add(1)) {
            conn.fin_pending = false;
            conn.fin_retries = 0;
            if conn.state == crate::net::TcpState::FinWait {
                conn.time_wait_until = crate::interrupts::get_system_ticks().saturating_add(crate::net::tcp::TCP_TIME_WAIT);
                conn.state = crate::net::TcpState::TimeWait;
            } else if conn.state == crate::net::TcpState::LastAck {
                conn.state = crate::net::TcpState::Closed;
            }
            return true;
        }
        false
    }

    pub fn due_fin_retransmit(&mut self, now: u64) -> Option<(u32, Ipv4Address, u16, u16, u32, u32, u16)> {
        for slot in self.entries.iter_mut() {
            if let Some(conn) = slot.as_mut() {
                if conn.fin_pending && now >= conn.fin_deadline {
                    if conn.fin_retries >= TCP_MAX_RETRIES {
                        conn.fin_pending = false;
                        conn.state = crate::net::TcpState::Closed;
                        continue;
                    }
                    conn.fin_retries += 1;
                    conn.fin_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS << conn.fin_retries);
                    return Some((conn.id, conn.remote_ip, conn.local_port, conn.remote_port, conn.fin_seq, conn.recv_seq, conn.rcv_wnd.min(u16::MAX as u32) as u16));
                }
            }
        }
        None
    }

    /// Releases every TCP connection owned by a terminating task.
    ///
    /// This is the TCP counterpart to SocketTable::release_owner and prevents
    /// fixed-size connection slots from surviving process termination.
    pub fn release_owner(&mut self, owner_task_id: u32) -> usize {
        let mut released = 0;
        for slot in self.entries.iter_mut() {
            if slot.as_ref().map(|conn| conn.owner_task_id == owner_task_id).unwrap_or(false) {
                *slot = None;
                released += 1;
            }
        }
        if released > 0 {
            if let Some(process) = crate::task::process::process_for_task(crate::task::TaskId(owner_task_id as u64)) {
                for _ in 0..released { crate::task::process::release_network_socket(process); }
            }
        }
        released
    }

    /// Reaps connections that reached the terminal Closed state.
    ///
    /// Called by the bounded network poll path so handshake/retransmission
    /// failures cannot permanently consume the fixed TCP connection table.
    /// Expires active TIME-WAIT entries only after the protocol hold period.
    ///
    /// TIME-WAIT connections remain addressable during the hold interval so a
    /// delayed duplicate segment cannot immediately collide with a new 4-tuple.
    pub fn reap_time_wait(&mut self, now: u64) -> usize {
        let mut expired = 0;
        for slot in self.entries.iter_mut() {
            if let Some(conn) = slot.as_mut() {
                if conn.state == crate::net::TcpState::TimeWait && now >= conn.time_wait_until {
                    conn.state = crate::net::TcpState::Closed;
                    conn.time_wait_until = 0;
                    expired += 1;
                }
            }
        }
        expired
    }

    pub fn reap_closed(&mut self) -> usize {
        let mut reaped = 0;
        let mut owners = [0u32; 8];
        for slot in self.entries.iter_mut() {
            if slot.as_ref().map(|conn| conn.state == crate::net::TcpState::Closed && !conn.fin_pending).unwrap_or(false) {
                if reaped < owners.len() {
                    owners[reaped] = slot.as_ref().unwrap().owner_task_id;
                }
                *slot = None;
                reaped += 1;
            }
        }
        for owner in owners.iter().take(reaped) {
            if let Some(process) = crate::task::process::process_for_task(crate::task::TaskId(*owner as u64)) {
                crate::task::process::release_network_socket(process);
            }
        }
        reaped
    }

    pub fn is_closed(&self, id: u32) -> bool {
        self.get(id).map(|c| c.state == crate::net::TcpState::Closed && !c.fin_pending).unwrap_or(false)
    }

    pub fn remove_closed(&mut self, id: u32, owner_task_id: u32) -> Result<(), NetError> {
        let slot = self.entries.iter_mut().find(|s| s.as_ref().map(|c| c.id == id).unwrap_or(false));
        match slot {
            Some(entry) if entry.as_ref().map(|c| c.owner_task_id == owner_task_id && c.state == crate::net::TcpState::Closed).unwrap_or(false) => {
                *entry = None;
                if let Some(process) = crate::task::process::process_for_task(crate::task::TaskId(owner_task_id as u64)) {
                    crate::task::process::release_network_socket(process);
                }
                Ok(())
            }
            Some(_) => Err(NetError::NotReady),
            None => Err(NetError::NoPacketAvailable),
        }
    }

    pub fn tx_payload(&self, id: u32) -> Option<&[u8]> {
        self.get(id).filter(|c| c.tx_pending).map(|c| &c.tx[..c.tx_len])
    }

    pub fn recv_owned(
        &mut self,
        id: u32,
        owner_task_id: u32,
        out: &mut [u8],
        caps: &CapabilityTable,
    ) -> Result<usize, NetError> {
        // Enforce capability authorization upfront to eliminate connection-ID enumeration oracles
        if !caps.has_capability_object(CapabilityType::NetworkReceive, Rights::READ, id as u64) {
            return Err(NetError::CapabilityDenied);
        }

        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if conn.owner_task_id != owner_task_id {
            return Err(NetError::CapabilityDenied);
        }

        if conn.rx_len == 0 {
            return Err(NetError::NoPacketAvailable);
        }

        // Consume exactly one invocation of the connection-matching NetworkReceive lease upon actual reception
        if !caps.authorize_capability_object(CapabilityType::NetworkReceive, Rights::READ, id as u64) {
            return Err(NetError::CapabilityDenied);
        }

        let n = out.len().min(conn.rx_len);
        out[..n].copy_from_slice(&conn.rx[..n]);
        conn.rx.copy_within(n..conn.rx_len, 0);
        conn.rx_len -= n;
        conn.rcv_wnd = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len) as u32;
        Ok(n)
    }

    pub fn close_owned(&mut self, id: u32, owner_task_id: u32) -> Result<(), NetError> {
        let slot = self.entries.iter_mut().find(|s| s.as_ref().map(|c| c.id == id).unwrap_or(false));
        match slot {
            Some(entry) if entry.as_ref().map(|c| c.owner_task_id == owner_task_id).unwrap_or(false) => {
                *entry = None;
                Ok(())
            }
            _ => Err(NetError::CapabilityDenied),
        }
    }
}


#[cfg(test)]
mod tcp_reliability_tests {
    use super::*;
    use crate::net::ipv4::Ipv4Address;

    #[test]
    fn active_close_holds_connection_until_time_wait_expiry() {
        let mut table = TcpConnectionTable::new();
        let id = table
            .open(7, 41000, Ipv4Address::LOCALHOST, 41001, 100)
            .expect("connection should open");

        {
            let conn = table.get_mut(id).expect("connection exists");
            conn.state = crate::net::TcpState::Established;
        }
        let fin_seq = table.arm_fin(id, 0).expect("FIN should arm");
        assert!(table.acknowledge_fin(id, fin_seq.wrapping_add(1)));

        let until = table.get(id).expect("TIME-WAIT entry remains").time_wait_until;
        assert_eq!(table.get(id).expect("entry remains").state, crate::net::TcpState::TimeWait);
        assert_eq!(table.reap_time_wait(until.saturating_sub(1)), 0);
        assert!(table.get(id).is_some());

        assert_eq!(table.reap_time_wait(until), 1);
        assert_eq!(table.reap_closed(), 1);
        assert!(table.get(id).is_none());
    }

    #[test]
    fn payload_retransmit_exhaustion_closes_connection_and_releases_slot() {
        let mut table = TcpConnectionTable::new();
        let id = table
            .open(7, 40000, Ipv4Address::LOCALHOST, 40001, 100)
            .expect("connection should open");

        table.get_mut(id).expect("connection exists").state = crate::net::TcpState::Established;
        table.arm_retransmit(id, 100, b"payload", 0).expect("arm retransmit");

        assert!(table.due_retransmit(500).is_some());
        assert!(table.due_retransmit(1500).is_some());
        assert!(table.due_retransmit(3500).is_some());

        assert!(table.due_retransmit(7500).is_none());
        let conn = table.get(id).expect("closed entry remains until reap");
        assert_eq!(conn.state, crate::net::TcpState::Closed);
        assert!(!conn.tx_pending);
        assert_eq!(conn.tx_len, 0);
        assert_eq!(conn.tx_deadline, 0);

        assert_eq!(table.reap_closed(), 1);
        assert!(table.get(id).is_none());
    }

    #[test]
    fn delegated_leases_network_paths_expiry_revocation_exhaustion_and_valid() {
        use crate::capability::{delegation, Capability, CapabilityId};
        use crate::task::TaskId;

        let task_id = TaskId(999);
        let now = crate::interrupts::get_system_ticks();

        let parent_sock = Capability {
            id: CapabilityId(101),
            cap_type: CapabilityType::NetworkSocket,
            rights: Rights::WRITE.union(Rights::GRANT),
            object_id: 0,
        };
        let parent_send = Capability {
            id: CapabilityId(102),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE.union(Rights::GRANT),
            object_id: 0,
        };
        let parent_recv = Capability {
            id: CapabilityId(103),
            cap_type: CapabilityType::NetworkReceive,
            rights: Rights::READ.union(Rights::GRANT),
            object_id: 0,
        };

        // 1. Single-use valid leases
        let lease_sock = delegation::DelegatedLease::new(
            8001, &parent_sock, TaskId(1), task_id, Rights::WRITE, 1, 100_000, now,
        ).expect("create lease_sock");
        delegation::register_lease(lease_sock.clone());

        let lease_send = delegation::DelegatedLease::new(
            8002, &parent_send, TaskId(1), task_id, Rights::WRITE, 1, 100_000, now,
        ).expect("create lease_send");
        delegation::register_lease(lease_send.clone());

        let lease_recv = delegation::DelegatedLease::new(
            8003, &parent_recv, TaskId(1), task_id, Rights::READ, 1, 100_000, now,
        ).expect("create lease_recv");
        delegation::register_lease(lease_recv.clone());

        let mut caps = CapabilityTable::new_for_task(task_id, 8);
        caps.insert(lease_sock.to_capability()).expect("insert lease_sock");
        caps.insert(lease_send.to_capability()).expect("insert lease_send");
        caps.insert(lease_recv.to_capability()).expect("insert lease_recv");

        // has_capability returns true without consuming
        assert!(caps.has_capability(CapabilityType::NetworkSocket, Rights::WRITE));
        assert!(caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        assert!(caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

        // Test create_socket single-use
        let mut sock_table = SocketTable::new();
        let sock_id = sock_table.create_socket(task_id.0 as u32, 45000, &caps)
            .expect("first create_socket must succeed");
        // Quota is now consumed (0 remaining) -> has_capability and second create_socket must fail
        assert!(!caps.has_capability(CapabilityType::NetworkSocket, Rights::WRITE));
        let second_sock = sock_table.create_socket(task_id.0 as u32, 45001, &caps);
        assert_eq!(second_sock.err(), Some(NetError::CapabilityDenied));

        // Test recv_from when empty does not consume
        let mut rx_buf = [0u8; 64];
        {
            let sock = sock_table.get_mut(sock_id).expect("sock exists");
            let empty_recv = sock.recv_from(&mut rx_buf, &caps);
            assert_eq!(empty_recv.err(), Some(NetError::NoPacketAvailable));
            assert!(caps.has_capability(CapabilityType::NetworkReceive, Rights::READ)); // Still unexhausted
        }

        // Test send_udp_datagram single-use
        let send_res1 = crate::net::send_udp_datagram(
            Ipv4Address::LOCALHOST, 45000, Ipv4Address::LOCALHOST, 45000, b"TEST", &caps,
        );
        assert!(send_res1.is_ok(), "first send_udp_datagram must succeed: {:?}", send_res1.err());
        assert!(!caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        let send_res2 = crate::net::send_udp_datagram(
            Ipv4Address::LOCALHOST, 45000, Ipv4Address::LOCALHOST, 45000, b"TEST", &caps,
        );
        assert_eq!(send_res2.err(), Some(NetError::CapabilityDenied));

        // Enqueue packet into socket for recv_from verification
        sock_table.get_mut(sock_id).unwrap().enqueue_rx(Ipv4Address::LOCALHOST, 45000, b"TEST").expect("enqueue");

        // Test recv_from single-use on queued packet
        {
            let sock = sock_table.get_mut(sock_id).expect("sock exists");
            let recv_res1 = sock.recv_from(&mut rx_buf, &caps);
            assert!(recv_res1.is_ok(), "first recv_from must succeed: {:?}", recv_res1.err());
            assert_eq!(&rx_buf[..recv_res1.unwrap().0], b"TEST");
            assert!(!caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

            // Second recv_from must fail with CapabilityDenied
            sock.enqueue_rx(Ipv4Address::LOCALHOST, 45000, b"EXTRA").expect("enqueue");
            let recv_res2 = sock.recv_from(&mut rx_buf, &caps);
            assert_eq!(recv_res2.err(), Some(NetError::CapabilityDenied));
        }

        // 2. Expired delegated leases across create_socket, send_udp_datagram, recv_from
        let lease_start = crate::interrupts::get_system_ticks();
        let exp_sock = delegation::DelegatedLease::new(
            8011, &parent_sock, TaskId(1), task_id, Rights::WRITE, 5, 10, lease_start,
        ).expect("exp_sock");
        delegation::register_lease(exp_sock.clone());
        let exp_send = delegation::DelegatedLease::new(
            8012, &parent_send, TaskId(1), task_id, Rights::WRITE, 5, 10, lease_start,
        ).expect("exp_send");
        delegation::register_lease(exp_send.clone());
        let exp_recv = delegation::DelegatedLease::new(
            8013, &parent_recv, TaskId(1), task_id, Rights::READ, 5, 10, lease_start,
        ).expect("exp_recv");
        delegation::register_lease(exp_recv.clone());

        // Advance system ticks past the 10-tick duration to expire the leases
        let prev_ticks = crate::interrupts::SYSTEM_TICKS.load(core::sync::atomic::Ordering::SeqCst);
        crate::interrupts::SYSTEM_TICKS.store(lease_start + 50, core::sync::atomic::Ordering::SeqCst);

        let mut exp_caps = CapabilityTable::new_for_task(task_id, 4);
        exp_caps.insert(exp_sock.to_capability()).unwrap();
        exp_caps.insert(exp_send.to_capability()).unwrap();
        exp_caps.insert(exp_recv.to_capability()).unwrap();

        assert!(!exp_caps.has_capability(CapabilityType::NetworkSocket, Rights::WRITE));
        assert!(!exp_caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        assert!(!exp_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

        assert_eq!(sock_table.create_socket(task_id.0 as u32, 45002, &exp_caps).err(), Some(NetError::CapabilityDenied));
        assert_eq!(crate::net::send_udp_datagram(Ipv4Address::LOCALHOST, 45000, Ipv4Address::LOCALHOST, 45000, b"X", &exp_caps).err(), Some(NetError::CapabilityDenied));
        assert_eq!(sock_table.get_mut(sock_id).unwrap().recv_from(&mut rx_buf, &exp_caps).err(), Some(NetError::CapabilityDenied));

        // Restore system ticks to avoid inter-test side effects
        crate::interrupts::SYSTEM_TICKS.store(prev_ticks, core::sync::atomic::Ordering::SeqCst);

        // 3. Revoked delegated leases across create_socket, send_udp_datagram, recv_from
        let current_now = crate::interrupts::get_system_ticks();
        let rev_sock = delegation::DelegatedLease::new(
            8021, &parent_sock, TaskId(1), task_id, Rights::WRITE, 5, 100_000, current_now,
        ).expect("rev_sock");
        delegation::register_lease(rev_sock.clone());
        delegation::revoke_lease(8021);

        let rev_send = delegation::DelegatedLease::new(
            8022, &parent_send, TaskId(1), task_id, Rights::WRITE, 5, 100_000, current_now,
        ).expect("rev_send");
        delegation::register_lease(rev_send.clone());
        delegation::revoke_lease(8022);

        let rev_recv = delegation::DelegatedLease::new(
            8023, &parent_recv, TaskId(1), task_id, Rights::READ, 5, 100_000, current_now,
        ).expect("rev_recv");
        delegation::register_lease(rev_recv.clone());
        delegation::revoke_lease(8023);

        let mut rev_caps = CapabilityTable::new_for_task(task_id, 4);
        rev_caps.insert(rev_sock.to_capability()).unwrap();
        rev_caps.insert(rev_send.to_capability()).unwrap();
        rev_caps.insert(rev_recv.to_capability()).unwrap();

        assert!(!rev_caps.has_capability(CapabilityType::NetworkSocket, Rights::WRITE));
        assert!(!rev_caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        assert!(!rev_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

        assert_eq!(sock_table.create_socket(task_id.0 as u32, 45003, &rev_caps).err(), Some(NetError::CapabilityDenied));
        assert_eq!(crate::net::send_udp_datagram(Ipv4Address::LOCALHOST, 45000, Ipv4Address::LOCALHOST, 45000, b"X", &rev_caps).err(), Some(NetError::CapabilityDenied));
        assert_eq!(sock_table.get_mut(sock_id).unwrap().recv_from(&mut rx_buf, &rev_caps).err(), Some(NetError::CapabilityDenied));

        // 4. Exhausted delegated leases across create_socket, send_udp_datagram, recv_from
        let exh_now = crate::interrupts::get_system_ticks();
        let exh_sock = delegation::DelegatedLease::new(
            8031, &parent_sock, TaskId(1), task_id, Rights::WRITE, 1, 100_000, exh_now,
        ).expect("exh_sock");
        delegation::register_lease(exh_sock.clone());
        delegation::consume_invocation(8031, Some(task_id), exh_now, Rights::WRITE).unwrap();

        let exh_send = delegation::DelegatedLease::new(
            8032, &parent_send, TaskId(1), task_id, Rights::WRITE, 1, 100_000, exh_now,
        ).expect("exh_send");
        delegation::register_lease(exh_send.clone());
        delegation::consume_invocation(8032, Some(task_id), exh_now, Rights::WRITE).unwrap();

        let exh_recv = delegation::DelegatedLease::new(
            8033, &parent_recv, TaskId(1), task_id, Rights::READ, 1, 100_000, exh_now,
        ).expect("exh_recv");
        delegation::register_lease(exh_recv.clone());
        delegation::consume_invocation(8033, Some(task_id), exh_now, Rights::READ).unwrap();

        let mut exh_caps = CapabilityTable::new_for_task(task_id, 4);
        exh_caps.insert(exh_sock.to_capability()).unwrap();
        exh_caps.insert(exh_send.to_capability()).unwrap();
        exh_caps.insert(exh_recv.to_capability()).unwrap();

        assert!(!exh_caps.has_capability(CapabilityType::NetworkSocket, Rights::WRITE));
        assert!(!exh_caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        assert!(!exh_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

        assert_eq!(sock_table.create_socket(task_id.0 as u32, 45004, &exh_caps).err(), Some(NetError::CapabilityDenied));
        assert_eq!(crate::net::send_udp_datagram(Ipv4Address::LOCALHOST, 45000, Ipv4Address::LOCALHOST, 45000, b"X", &exh_caps).err(), Some(NetError::CapabilityDenied));
        assert_eq!(sock_table.get_mut(sock_id).unwrap().recv_from(&mut rx_buf, &exh_caps).err(), Some(NetError::CapabilityDenied));
    }

    #[test]
    fn tcp_receive_capability_and_lease_enforcement() {
        use crate::capability::{delegation, Capability, CapabilityId};
        use crate::task::TaskId;

        let task_id = TaskId(999);

        let parent_recv = Capability {
            id: CapabilityId(301),
            cap_type: CapabilityType::NetworkReceive,
            rights: Rights::READ.union(Rights::GRANT),
            object_id: 0,
        };

        let mut table = TcpConnectionTable::new();
        let conn_id = table.open(task_id.0 as u32, 52000, Ipv4Address::LOCALHOST, 52001, 1000)
            .expect("open connection");
        table.get_mut(conn_id).expect("connection exists").state = crate::net::TcpState::Established;

        let mut out = [0u8; 64];

        // 1. No capability: empty capability table must be denied
        {
            let empty_caps = CapabilityTable::new_for_task(task_id, 4);
            table.enqueue_rx(conn_id, task_id.0 as u32, b"TEST_PAYLOAD").expect("enqueue");
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &empty_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "No capability must be denied");
        }

        // 2. Wrong capability: table holding only NetworkSend/NetworkSocket must be denied
        {
            let wrong_cap = Capability {
                id: CapabilityId(302),
                cap_type: CapabilityType::NetworkSend,
                rights: Rights::WRITE.union(Rights::READ),
                object_id: 0,
            };
            let mut wrong_caps = CapabilityTable::new_for_task(task_id, 4);
            wrong_caps.insert(wrong_cap).unwrap();
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &wrong_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Wrong capability must be denied");
        }

        // 3. Insufficient rights: NetworkReceive with only WRITE rights (missing READ) must be denied
        {
            let no_read_cap = Capability {
                id: CapabilityId(303),
                cap_type: CapabilityType::NetworkReceive,
                rights: Rights::WRITE,
                object_id: 0,
            };
            let mut no_read_caps = CapabilityTable::new_for_task(task_id, 4);
            no_read_caps.insert(no_read_cap).unwrap();
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &no_read_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Insufficient rights must be denied");
        }

        // 4. Expired delegated lease: lease whose ticks expired must be denied
        {
            let lease_start = crate::interrupts::get_system_ticks();
            let exp_lease = delegation::DelegatedLease::new(
                9011, &parent_recv, TaskId(1), task_id, Rights::READ, 5, 10, lease_start,
            ).expect("exp_lease");
            delegation::register_lease(exp_lease.clone());

            let prev_ticks = crate::interrupts::SYSTEM_TICKS.load(core::sync::atomic::Ordering::SeqCst);
            crate::interrupts::SYSTEM_TICKS.store(lease_start + 50, core::sync::atomic::Ordering::SeqCst);

            let mut exp_caps = CapabilityTable::new_for_task(task_id, 4);
            exp_caps.insert(exp_lease.to_capability()).unwrap();

            assert!(!exp_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &exp_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Expired lease must be denied");

            crate::interrupts::SYSTEM_TICKS.store(prev_ticks, core::sync::atomic::Ordering::SeqCst);
        }

        // 5. Revoked delegated lease: revoked lease must be denied
        {
            let rev_now = crate::interrupts::get_system_ticks();
            let rev_lease = delegation::DelegatedLease::new(
                9021, &parent_recv, TaskId(1), task_id, Rights::READ, 5, 100_000, rev_now,
            ).expect("rev_lease");
            delegation::register_lease(rev_lease.clone());
            delegation::revoke_lease(9021);

            let mut rev_caps = CapabilityTable::new_for_task(task_id, 4);
            rev_caps.insert(rev_lease.to_capability()).unwrap();

            assert!(!rev_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &rev_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Revoked lease must be denied");
        }

        // 6. Exhausted delegated lease: lease with 0 remaining invocations must be denied
        {
            let exh_now = crate::interrupts::get_system_ticks();
            let exh_lease = delegation::DelegatedLease::new(
                9031, &parent_recv, TaskId(1), task_id, Rights::READ, 1, 100_000, exh_now,
            ).expect("exh_lease");
            delegation::register_lease(exh_lease.clone());
            delegation::consume_invocation(9031, Some(task_id), exh_now, Rights::READ).unwrap();

            let mut exh_caps = CapabilityTable::new_for_task(task_id, 4);
            exh_caps.insert(exh_lease.to_capability()).unwrap();

            assert!(!exh_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &exh_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Exhausted lease must be denied");
        }

        // 7. Wrong grantee: lease granted to TaskId(555) cannot be authorized by TaskId(999)
        {
            let wg_now = crate::interrupts::get_system_ticks();
            let wg_lease = delegation::DelegatedLease::new(
                9041, &parent_recv, TaskId(1), TaskId(555), Rights::READ, 5, 100_000, wg_now,
            ).expect("wg_lease");
            delegation::register_lease(wg_lease.clone());

            let mut wg_caps = CapabilityTable::new_for_task(task_id, 4);
            wg_caps.insert(wg_lease.to_capability()).unwrap();

            assert!(!wg_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));
            let res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &wg_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Wrong grantee must be denied");
        }

        // 8. Connection ownership check: caller task ID mismatch must be denied
        {
            let valid_now = crate::interrupts::get_system_ticks();
            let valid_lease = delegation::DelegatedLease::new(
                9051, &parent_recv, TaskId(1), task_id, Rights::READ, 2, 100_000, valid_now,
            ).expect("valid_lease");
            delegation::register_lease(valid_lease.clone());

            let mut valid_caps = CapabilityTable::new_for_task(task_id, 4);
            valid_caps.insert(valid_lease.to_capability()).unwrap();

            let other_task = (task_id.0 + 1) as u32;
            let res = table.recv_owned(conn_id, other_task, &mut out, &valid_caps);
            assert_eq!(res.err(), Some(NetError::CapabilityDenied), "Wrong connection owner must be denied");
        }

        // 9. Empty rx queue: does not consume quota on NoPacketAvailable
        {
            // Drain current rx queue first
            table.get_mut(conn_id).unwrap().rx_len = 0;

            let single_now = crate::interrupts::get_system_ticks();
            let single_lease = delegation::DelegatedLease::new(
                9061, &parent_recv, TaskId(1), task_id, Rights::READ, 1, 100_000, single_now,
            ).expect("single_lease");
            delegation::register_lease(single_lease.clone());

            let mut single_caps = CapabilityTable::new_for_task(task_id, 4);
            single_caps.insert(single_lease.to_capability()).unwrap();

            assert!(single_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));
            let empty_res = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &single_caps);
            assert_eq!(empty_res.err(), Some(NetError::NoPacketAvailable));
            // Quota must NOT be consumed
            assert!(single_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

            // 10. Valid single-use receive: succeeds and consumes the single invocation
            table.enqueue_rx(conn_id, task_id.0 as u32, b"HELLO_TCP").expect("enqueue");
            let recv_res1 = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &single_caps);
            assert!(recv_res1.is_ok(), "First receive must succeed: {:?}", recv_res1.err());
            let n = recv_res1.unwrap();
            assert_eq!(&out[..n], b"HELLO_TCP");

            // Lease is now exhausted
            assert!(!single_caps.has_capability(CapabilityType::NetworkReceive, Rights::READ));

            // 11. Second receive is denied after single-use lease consumed
            table.enqueue_rx(conn_id, task_id.0 as u32, b"EXTRA_TCP").expect("enqueue");
            let recv_res2 = table.recv_owned(conn_id, task_id.0 as u32, &mut out, &single_caps);
            assert_eq!(recv_res2.err(), Some(NetError::CapabilityDenied), "Second receive must be denied after single-use lease exhaustion");
        }
    }

    #[test]
    fn tcp_recv_connection_object_scoping() {
        use crate::capability::{delegation, Capability, CapabilityId};
        use crate::task::TaskId;

        let task_id = TaskId(998);
        let mut table = TcpConnectionTable::new();

        let conn_a = table.open(task_id.0 as u32, 53000, Ipv4Address::LOCALHOST, 53001, 1000)
            .expect("open conn_a");
        table.get_mut(conn_a).unwrap().state = crate::net::TcpState::Established;

        let conn_b = table.open(task_id.0 as u32, 54000, Ipv4Address::LOCALHOST, 54001, 1000)
            .expect("open conn_b");
        table.get_mut(conn_b).unwrap().state = crate::net::TcpState::Established;

        table.enqueue_rx(conn_a, task_id.0 as u32, b"PAYLOAD_A").expect("enqueue conn_a");
        table.enqueue_rx(conn_b, task_id.0 as u32, b"PAYLOAD_B").expect("enqueue conn_b");

        // Mint a parent capability scoped specifically to connection A (object_id = conn_a as u64)
        let parent_recv_a = Capability {
            id: CapabilityId(551),
            cap_type: CapabilityType::NetworkReceive,
            rights: Rights::READ.union(Rights::GRANT),
            object_id: conn_a as u64,
        };

        let now = crate::interrupts::get_system_ticks();
        let lease_a = delegation::DelegatedLease::new(
            9091, &parent_recv_a, TaskId(1), task_id, Rights::READ, 10, 10_000, now,
        ).expect("create lease for conn_a");
        delegation::register_lease(lease_a.clone());

        let mut caps = CapabilityTable::new_for_task(task_id, 4);
        caps.insert(lease_a.to_capability()).expect("insert delegated lease");

        let mut out = [0u8; 64];

        // 1. Attempting to receive from connection B using lease scoped to connection A must be DENIED
        let res_b = table.recv_owned(conn_b, task_id.0 as u32, &mut out, &caps);
        assert_eq!(res_b.err(), Some(NetError::CapabilityDenied),
                   "Lease scoped to connection A must not authorize receive from connection B");

        // 2. Receiving from connection A using lease scoped to connection A must SUCCEED
        let res_a = table.recv_owned(conn_a, task_id.0 as u32, &mut out, &caps);
        assert!(res_a.is_ok(), "Lease scoped to connection A must authorize receive from connection A, got: {:?}", res_a.err());
        let n = res_a.unwrap();
        assert_eq!(&out[..n], b"PAYLOAD_A");
    }

    #[test]
    fn accept_rx_segment_fin_and_overlap_handling() {
        let mut table = TcpConnectionTable::new();
        let id = table
            .open(7, 42000, Ipv4Address::LOCALHOST, 42001, 100)
            .expect("connection should open");

        let initial_recv_seq = 5000u32;
        {
            let conn = table.get_mut(id).expect("connection exists");
            conn.state = crate::net::TcpState::Established;
            conn.recv_seq = initial_recv_seq;
        }

        // (d) overlapping segment with leading trim -> correct byte count
        // Send segment at seq 4995 with 10 bytes (bytes 4995..5005).
        // Bytes 4995..5000 should be trimmed; only 5 bytes (5000..5005) accepted.
        let payload = [10, 11, 12, 13, 14, 15, 16, 17, 18, 19];
        let n = table
            .accept_rx_segment(id, 4995, &payload, false)
            .expect("overlapping segment should succeed");
        assert_eq!(n, 5, "leading overlap should be trimmed to 5 bytes");
        {
            let conn = table.get(id).expect("connection exists");
            assert_eq!(conn.recv_seq, 5005);
            assert_eq!(conn.rx_len, 5);
            assert_eq!(&conn.rx[..5], &[15, 16, 17, 18, 19]);
            assert_eq!(conn.state, crate::net::TcpState::Established);
        }

        // (c) fully duplicate segment -> Ok(0), state unchanged
        // Retransmission of seq 4995 with 10 bytes (now entirely <= recv_seq 5005)
        let dup_n = table
            .accept_rx_segment(id, 4995, &payload, false)
            .expect("duplicate segment should return Ok(0)");
        assert_eq!(dup_n, 0, "duplicate segment must yield 0 bytes");
        {
            let conn = table.get(id).expect("connection exists");
            assert_eq!(conn.recv_seq, 5005, "recv_seq must not advance on duplicate");
            assert_eq!(conn.rx_len, 5, "rx_len must not change on duplicate");
            assert_eq!(conn.state, crate::net::TcpState::Established, "state must remain Established");
        }

        // Fully duplicate empty segment without FIN -> Ok(0), state unchanged
        let dup_empty = table
            .accept_rx_segment(id, 5005, &[], false)
            .expect("empty duplicate segment should return Ok(0)");
        assert_eq!(dup_empty, 0);
        {
            let conn = table.get(id).expect("connection exists");
            assert_eq!(conn.recv_seq, 5005);
            assert_eq!(conn.state, crate::net::TcpState::Established);
        }

        // (b) FIN appended to in-order payload -> CloseWait
        // Send 4 bytes at seq 5005 with fin=true.
        let in_order_payload = [20, 21, 22, 23];
        let n_b = table
            .accept_rx_segment(id, 5005, &in_order_payload, true)
            .expect("payload + FIN should succeed");
        assert_eq!(n_b, 4);
        {
            let conn = table.get(id).expect("connection exists");
            // recv_seq should advance by 4 (payload) + 1 (FIN) = 5010
            assert_eq!(conn.recv_seq, 5010);
            assert_eq!(conn.rx_len, 9);
            assert_eq!(&conn.rx[5..9], &[20, 21, 22, 23]);
            assert_eq!(conn.state, crate::net::TcpState::CloseWait, "state must transition to CloseWait");
        }

        // (a) zero-length FIN at seq == recv_seq -> CloseWait and recv_seq + 1
        let id2 = table
            .open(7, 43000, Ipv4Address::LOCALHOST, 43001, 100)
            .expect("connection 2 should open");
        let initial_recv_seq_2 = 8000u32;
        {
            let conn = table.get_mut(id2).expect("connection exists");
            conn.state = crate::net::TcpState::Established;
            conn.recv_seq = initial_recv_seq_2;
        }

        let n_a = table
            .accept_rx_segment(id2, initial_recv_seq_2, &[], true)
            .expect("zero-length FIN should succeed");
        assert_eq!(n_a, 0);
        {
            let conn = table.get(id2).expect("connection exists");
            assert_eq!(conn.recv_seq, initial_recv_seq_2.wrapping_add(1), "recv_seq must advance by 1 for zero-length FIN");
            assert_eq!(conn.state, crate::net::TcpState::CloseWait, "state must transition to CloseWait on zero-length FIN");
        }
    }
}
