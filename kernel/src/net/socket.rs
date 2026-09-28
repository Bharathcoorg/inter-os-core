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
pub const TCP_RX_BUF_SIZE: usize = 2048;
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

        if port == 0 { return Err(NetError::InvalidPort); }
        if self.sockets.iter().flatten().any(|sock| sock.is_bound && sock.local_port == port) {
            return Err(NetError::PortInUse);
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
    /// Releases every socket owned by a terminated task.
    pub fn release_owner(&mut self, owner_task_id: u32) -> usize {
        let mut released = 0;
        for slot in self.sockets.iter_mut() {
            if slot.as_ref().map(|sock| sock.owner_task_id == owner_task_id).unwrap_or(false) {
                *slot = None;
                released += 1;
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
        if self.entries.iter().flatten().any(|conn| {
            conn.local_port == local_port
                && conn.remote_ip == remote_ip
                && conn.remote_port == remote_port
                && conn.state != crate::net::TcpState::Closed
        }) {
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
                    syn_deadline: TCP_RETRANSMIT_TICKS,
                    syn_retries: 0,
                    time_wait_until: 0,
                });
                return Ok(id);
            }
        }
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
        if crate::net::tcp::seq_lt(seq, conn.recv_seq) { return Ok(0); }
        if seq != conn.recv_seq { return Err(NetError::NoPacketAvailable); }
        let room = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len);
        let n = payload.len().min(room);
        if n != payload.len() && !payload.is_empty() { return Err(NetError::QueueFull); }
        conn.rx[conn.rx_len..conn.rx_len + n].copy_from_slice(payload);
        conn.rx_len += n;
        conn.recv_seq = conn.recv_seq.wrapping_add(n as u32);
        conn.rcv_wnd = TCP_RX_BUF_SIZE.saturating_sub(conn.rx_len) as u32;
        if fin && n == payload.len() {
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
                        conn.tx_pending = false;
                        conn.tx_len = 0;
                        conn.tx_retries = 0;
                        conn.tx_deadline = 0;
                        conn.state = crate::net::TcpState::Closed;
                        continue;
                    }
                    conn.tx_retries += 1;
                    conn.tx_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS << conn.tx_retries);
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
                        conn.syn_pending = false;
                        conn.state = crate::net::TcpState::Closed;
                        continue;
                    }
                    conn.syn_retries += 1;
                    conn.syn_deadline = now.saturating_add(TCP_RETRANSMIT_TICKS << conn.syn_retries);
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
        for slot in self.entries.iter_mut() {
            if slot.as_ref().map(|conn| conn.state == crate::net::TcpState::Closed && !conn.fin_pending).unwrap_or(false) {
                *slot = None;
                reaped += 1;
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
                Ok(())
            }
            Some(_) => Err(NetError::NotReady),
            None => Err(NetError::NoPacketAvailable),
        }
    }

    pub fn tx_payload(&self, id: u32) -> Option<&[u8]> {
        self.get(id).filter(|c| c.tx_pending).map(|c| &c.tx[..c.tx_len])
    }

    pub fn recv_owned(&mut self, id: u32, owner_task_id: u32, out: &mut [u8]) -> Result<usize, NetError> {
        let conn = self.get_mut(id).ok_or(NetError::NoPacketAvailable)?;
        if conn.owner_task_id != owner_task_id { return Err(NetError::CapabilityDenied); }
        if conn.rx_len == 0 { return Err(NetError::NoPacketAvailable); }
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
}

