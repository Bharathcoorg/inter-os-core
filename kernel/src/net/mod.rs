//! Inter OS Pure no_std Network Subsystem
//!
//! Exposes Ethernet II, ARP, IPv4, UDP, Loopback device, and capability-governed sockets.
//! Enforces Zero-Trust Principle #2: Network operations require explicit capability tokens.

pub mod arp;
pub mod device;
pub mod dns;
pub mod dhcp;
pub mod e1000;
pub mod tcp;
pub mod tls;
pub mod ethernet;
pub mod ipv4;
pub mod ipv6;
pub mod socket;
pub mod udp;
pub mod wireless;
pub mod route;

pub use arp::{ArpOpcode, ArpPacket, ArpTable};
pub use device::{LoopbackDevice, NetDeviceInfo, NetError, NetworkBackend, NetworkDevice, NetworkDeviceManager, NETWORK_DEVICE, LOOPBACK_DEVICE, ENABLE_E1000_BINDING};
pub use e1000::E1000Device;
pub use dns::{encode_query as dns_encode_query, parse_a_answer, DnsAnswer, DnsQuestion, DnsResolver, DNS_SERVER};
pub use tcp::{next_state as tcp_next_state, seq_ge, seq_gt, seq_le, seq_lt, valid_established_segment, valid_syn_ack, TcpSegment, TcpState};
use crate::task::TaskId;
pub use tls::{ContentType as TlsContentType, TlsRecord};
pub use ethernet::{EtherType, EthernetFrame, MacAddress};
pub use ipv4::{calculate_ipv4_checksum, verify_ipv4_checksum, IpProtocol, Ipv4Address, Ipv4Packet};
pub use socket::{SocketId, SocketTable, TcpConnection, TcpConnectionTable, UdpSocket, TCP_MAX_RETRIES, TCP_RETRANSMIT_TICKS};
pub use udp::UdpDatagram;
pub use wireless::{WifiState, WirelessManager, WIRELESS_MANAGER};
pub use route::{Ipv4Cidr, Route, RoutingTable, MAX_ROUTES};

use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::serial_println;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDropKind {
    NonIpv4Ethertype = 0,
    MalformedChecksumOrLength = 1,
    UnknownL4Protocol = 2,
    PortOrConnectionMismatch = 3,
    RxRingBackpressure = 4,
    Ipv4Fragment = 5,
}

pub struct NetDropStats {
    pub non_ipv4_ethertype: AtomicU64,
    pub malformed_checksum_length: AtomicU64,
    pub unknown_l4_protocol: AtomicU64,
    pub port_conn_mismatch: AtomicU64,
    pub rx_backpressure: AtomicU64,
    pub ipv4_fragment: AtomicU64,
    logged_mask: core::sync::atomic::AtomicU32,
}

impl NetDropStats {
    pub const fn new() -> Self {
        Self {
            non_ipv4_ethertype: AtomicU64::new(0),
            malformed_checksum_length: AtomicU64::new(0),
            unknown_l4_protocol: AtomicU64::new(0),
            port_conn_mismatch: AtomicU64::new(0),
            rx_backpressure: AtomicU64::new(0),
            ipv4_fragment: AtomicU64::new(0),
            logged_mask: core::sync::atomic::AtomicU32::new(0),
        }
    }

    pub fn record_drop(&self, kind: NetDropKind, detail: &str) {
        let counter = match kind {
            NetDropKind::NonIpv4Ethertype => &self.non_ipv4_ethertype,
            NetDropKind::MalformedChecksumOrLength => &self.malformed_checksum_length,
            NetDropKind::UnknownL4Protocol => &self.unknown_l4_protocol,
            NetDropKind::PortOrConnectionMismatch => &self.port_conn_mismatch,
            NetDropKind::RxRingBackpressure => &self.rx_backpressure,
            NetDropKind::Ipv4Fragment => &self.ipv4_fragment,
        };
        let prev = counter.fetch_add(1, Ordering::Relaxed);
        let bit = 1 << (kind as u32);
        if prev == 0 && (self.logged_mask.fetch_or(bit, Ordering::Relaxed) & bit) == 0 {
            serial_println!("[NET DROP FIRST] kind={:?} detail='{}'", kind, detail);
        }
    }
}

pub static DROP_STATS: NetDropStats = NetDropStats::new();

pub fn record_net_drop(kind: NetDropKind, detail: &str) {
    DROP_STATS.record_drop(kind, detail);
}

pub fn log_net_drop(reason: &str) {
    let kind = match reason {
        "ipv4_fragment_discarded_no_reassembly" => NetDropKind::Ipv4Fragment,
        "tcp_checksum_mismatch" | "tcp_parse_failed" | "udp_parse_failed" | "ipv4_parse_failed" => NetDropKind::MalformedChecksumOrLength,
        "tcp_no_matching_connection" => NetDropKind::PortOrConnectionMismatch,
        _ => NetDropKind::UnknownL4Protocol,
    };
    record_net_drop(kind, reason);
}

pub fn net_drop_stats() -> (u64, u64, u64, u64, u64, u64) {
    (
        DROP_STATS.non_ipv4_ethertype.load(Ordering::Relaxed),
        DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed),
        DROP_STATS.unknown_l4_protocol.load(Ordering::Relaxed),
        DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed),
        DROP_STATS.rx_backpressure.load(Ordering::Relaxed),
        DROP_STATS.ipv4_fragment.load(Ordering::Relaxed),
    )
}

pub const DEFAULT_HOST_IP: Ipv4Address = Ipv4Address([10, 0, 2, 15]);
pub const DEFAULT_HOST_MAC: MacAddress = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
/// QEMU user-mode (SLIRP) virtual gateway. A real ARP request addressed here is
/// answered by QEMU with a real reply, which is the end-to-end proof that a
/// packet left the guest and a response came back.
pub const GATEWAY_IP: Ipv4Address = Ipv4Address([10, 0, 2, 2]);

/// Set once the first real ARP reply from the gateway has been observed, so the
/// proof line is logged exactly once rather than on every network poll.
static LIVE_GATEWAY_ARP_LOGGED: AtomicBool = AtomicBool::new(false);

/// Global dynamic ARP resolution cache.
pub static ARP_TABLE: Mutex<ArpTable> = Mutex::new(ArpTable::new());
pub const ARP_REQUEST_RETRY_TICKS: u64 = 100;

#[derive(Copy, Clone)]
struct ArpRequestState {
    ip: Ipv4Address,
    requested_at: u64,
    valid: bool,
}

impl ArpRequestState {
    const EMPTY: Self = Self { ip: Ipv4Address::UNSPECIFIED, requested_at: 0, valid: false };
}

static ARP_REQUESTS: Mutex<[ArpRequestState; 8]> = Mutex::new([ArpRequestState::EMPTY; 8]);

/// Global capability-guarded socket table.
pub static SOCKET_TABLE: Mutex<SocketTable> = Mutex::new(SocketTable::new());
pub static TCP_TABLE: Mutex<TcpConnectionTable> = Mutex::new(TcpConnectionTable::new());

/// Releases network endpoints owned by a terminating task.
///
/// Global routing, ARP, and DNS cache state is intentionally preserved because
/// those resources are system-wide rather than task-owned.
pub fn cleanup_task(task_id: TaskId) {
    SOCKET_TABLE.lock().release_owner(task_id.0 as u32);
    TCP_TABLE.lock().release_owner(task_id.0 as u32);
}

/// Global bounded DNS resolver state. Keeping this state at network-subsystem
/// scope makes the resolver cache effective across syscall invocations while
/// preserving the fixed in-flight query and cache bounds.
pub static DNS_RESOLVER: Mutex<DnsResolver> = Mutex::new(DnsResolver::new());

fn transmit_arp_request(target_ip: Ipv4Address) -> Result<(), NetError> {
    let info = NETWORK_DEVICE.lock().info();
    let packet = ArpPacket {
        opcode: ArpOpcode::Request,
        sender_mac: info.mac,
        sender_ip: DEFAULT_HOST_IP,
        target_mac: MacAddress::ZERO,
        target_ip,
    };
    let mut arp_buf = [0u8; 28];
    let arp_len = packet.serialize(&mut arp_buf).map_err(|_| NetError::BufferTooSmall)?;
    let frame = EthernetFrame {
        dst_mac: MacAddress::BROADCAST,
        src_mac: info.mac,
        ethertype: EtherType::Arp,
        payload: &arp_buf[..arp_len],
    };
    let mut frame_buf = [0u8; 1514];
    let len = frame.serialize(&mut frame_buf).map_err(|_| NetError::BufferTooSmall)?;
    NETWORK_DEVICE.lock().transmit(&frame_buf[..len])
}

fn resolve_destination_mac(dst_ip: Ipv4Address) -> Result<MacAddress, NetError> {
    if dst_ip.is_loopback() {
        return Ok(MacAddress([0, 0, 0, 0, 0, 1]));
    }
    let now = crate::interrupts::get_system_ticks();
    if let Some(mac) = ARP_TABLE.lock().lookup_at(dst_ip, now) {
        return Ok(mac);
    }
    // Fall back to existing cached mapping while triggering background refresh
    if let Some(mac) = ARP_TABLE.lock().lookup(dst_ip) {
        let _ = transmit_arp_request(dst_ip);
        return Ok(mac);
    }
    let mut requests = ARP_REQUESTS.lock();
    if let Some(entry) = requests.iter().find(|entry| entry.valid && entry.ip == dst_ip) {
        if now.saturating_sub(entry.requested_at) < ARP_REQUEST_RETRY_TICKS {
            return Err(NetError::NoPacketAvailable);
        }
    }
    let slot_index = requests.iter().position(|entry| !entry.valid || entry.ip == dst_ip).unwrap_or(0);
    requests[slot_index] = ArpRequestState { ip: dst_ip, requested_at: now, valid: true };
    drop(requests);
    transmit_arp_request(dst_ip)?;
    Err(NetError::NoPacketAvailable)
}

/// Hands one serialized Ethernet frame to the backend that owns its destination.
///
/// Loopback destinations must never reach the controller: the NIC would put a
/// 127.0.0.1 frame on the wire, where nothing delivers it back. Those frames go
/// to the kernel loopback queue, which `poll_network_interface` drains.
fn transmit_frame(dst_ip: Ipv4Address, frame: &[u8]) -> Result<(), NetError> {
    if dst_ip.is_loopback() {
        LOOPBACK_DEVICE.lock().transmit(frame)
    } else {
        NETWORK_DEVICE.lock().transmit(frame)
    }
}

/// Transmits one real ARP request for the default gateway through the freshly
/// bound Ethernet backend and logs that a real frame was handed to the NIC.
///
/// Called from `NetworkDeviceManager::attach_e1000` immediately after the E1000
/// is bound, while the `NETWORK_DEVICE` lock is already held, so it drives the
/// controller through the passed `&mut` manager and never re-locks. Receipt of
/// the matching reply is detected and logged in `poll_network_interface`.
pub fn emit_gateway_arp_request(manager: &mut NetworkDeviceManager) {
    let info = manager.info();
    let request = ArpPacket {
        opcode: ArpOpcode::Request,
        sender_mac: info.mac,
        sender_ip: DEFAULT_HOST_IP,
        target_mac: MacAddress::ZERO,
        target_ip: GATEWAY_IP,
    };
    let mut arp_buf = [0u8; 28];
    let Ok(arp_len) = request.serialize(&mut arp_buf) else {
        return;
    };
    let frame = EthernetFrame {
        dst_mac: MacAddress::BROADCAST,
        src_mac: info.mac,
        ethertype: EtherType::Arp,
        payload: &arp_buf[..arp_len],
    };
    let mut frame_buf = [0u8; 1514];
    let Ok(frame_len) = frame.serialize(&mut frame_buf) else {
        return;
    };
    match manager.transmit(&frame_buf[..frame_len]) {
        Ok(()) => serial_println!(
            "[NET LIVE] Real ARP who-has {}.{}.{}.{} handed to the {:?} NIC as a {} byte Ethernet frame; awaiting gateway reply.",
            GATEWAY_IP.0[0], GATEWAY_IP.0[1], GATEWAY_IP.0[2], GATEWAY_IP.0[3],
            info.kind, frame_len
        ),
        Err(e) => serial_println!("[NET LIVE] Real ARP probe transmit failed: {:?}", e),
    }
}

/// Sends a UDP datagram over the network stack, verifying caller's capability.
pub fn send_udp_datagram(
    src_ip: Ipv4Address,
    src_port: u16,
    dst_ip: Ipv4Address,
    dst_port: u16,
    payload: &[u8],
    caps: &CapabilityTable,
) -> Result<(), NetError> {
    send_udp_datagram_authorized(src_ip, src_port, dst_ip, dst_port, payload, caps, true)
}

/// Authorizes one UDP transmission and then debits the caller's background
/// network budget, in exactly that order.
///
/// WHY this order is a security property rather than a style choice: the
/// background budget is an allowance for bytes that actually went on the wire.
/// If the debit ran first, a send that capability authorization refuses would
/// still have consumed budget for traffic that never existed, which lets a
/// quota-exhausted or unprivileged caller starve its own process allowance with
/// frames the kernel rejected.
///
/// `consume_quota` is `false` only for internal retransmissions of an
/// already-authorized logical query. Those must still prove the capability is
/// held and unrevoked, but must not spend a second invocation of a
/// single-invocation lease.
///
/// `caller_task` is the scheduler's current task; it is `None` outside a
/// scheduled context, where no process budget exists to debit.
pub(crate) fn authorize_and_debit_network_send(
    caps: &CapabilityTable,
    caller_task: Option<TaskId>,
    frame_len: usize,
    consume_quota: bool,
) -> Result<(), NetError> {
    if consume_quota {
        if !caps.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE) {
            return Err(NetError::CapabilityDenied);
        }
    } else if !caps.has_authorized_or_retransmit_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return Err(NetError::CapabilityDenied);
    }

    if let Some(task) = caller_task {
        crate::task::process::consume_background_network(task, frame_len)
            .map_err(|_| NetError::QueueFull)?;
    }
    Ok(())
}

/// Sends a UDP datagram over the network stack with explicit control over lease quota consumption.
///
/// When `consume_quota` is `true`, one invocation of a delegated `NetworkSend`
/// lease is spent on the transmission. When `false` the caller is retransmitting
/// an already-authorized logical query: authority is still verified and must be
/// unrevoked and unexpired, but a lease that has already spent its invocation
/// authorizes the retransmission rather than losing the retry.
pub fn send_udp_datagram_authorized(
    src_ip: Ipv4Address,
    src_port: u16,
    dst_ip: Ipv4Address,
    dst_port: u16,
    payload: &[u8],
    caps: &CapabilityTable,
    consume_quota: bool,
) -> Result<(), NetError> {
    // 1. Enforce Zero-Trust Capability Invariant before any work is done.
    //
    // WHY the retransmission arm is relaxation rather than bypass: this gate and
    // the one in `authorize_and_debit_network_send` MUST use the same predicate.
    // When this gate used the strict per-logical-operation check, a lease whose
    // single invocation was spent by the initial query was already refused here,
    // which made every DNS retransmission fail closed and left the relaxed check
    // downstream unreachable.
    let holds_authority = if consume_quota {
        caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE)
    } else {
        caps.has_authorized_or_retransmit_capability(CapabilityType::NetworkSend, Rights::WRITE)
    };
    if !holds_authority {
        return Err(NetError::CapabilityDenied);
    }

    // 2. Serialize UDP Layer
    let mut udp_buf = [0u8; 1024];
    let udp_len = UdpDatagram::serialize(src_port, dst_port, payload, &mut udp_buf)
        .map_err(|_| NetError::BufferTooSmall)?;

    // 3. Serialize IPv4 Layer
    let mut ip_buf = [0u8; 1200];
    let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Udp, 64, &udp_buf[0..udp_len], &mut ip_buf)
        .map_err(|_| NetError::BufferTooSmall)?;

    // 4. Resolve MAC Destination (ARP or Loopback)
    let dst_mac = resolve_destination_mac(dst_ip)?;

    // 5. Serialize Ethernet Frame
    let mut eth_buf = [0u8; 1514];
    let frame = EthernetFrame {
        dst_mac,
        src_mac: NETWORK_DEVICE.lock().info().mac,
        ethertype: EtherType::Ipv4,
        payload: &ip_buf[0..ip_len],
    };
    let eth_len = frame.serialize(&mut eth_buf).map_err(|_| NetError::BufferTooSmall)?;

    // 6. Authorize capability FIRST, then debit the process-scoped background
    //    network budget. A refused send must never consume byte allowance.
    let caller_task = crate::task::SCHEDULER.lock().current_task_id();
    authorize_and_debit_network_send(caps, caller_task, eth_len, consume_quota)?;

    // 7. Transmit into the loopback queue or the hardware NIC
    transmit_frame(dst_ip, &eth_buf[0..eth_len])
}

pub fn transmit_tcp_segment(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, flags: u16) -> Result<(), NetError> {
    transmit_tcp_segment_with_window(src_ip, src_port, dst_ip, dst_port, seq, ack, flags, 65535)
}

pub fn transmit_tcp_segment_with_window(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, flags: u16, window: u16) -> Result<(), NetError> {
    if flags & 0x02 != 0 && flags & 0x10 == 0 && window == 0 { return Err(NetError::InvalidPacket); }
    let segment = TcpSegment { src_port, dst_port, seq, ack, flags, window, payload: &[] };
    let mut tcp_buf = [0u8; 64];
    let tcp_len = crate::net::tcp::serialize_with_checksum(&segment, src_ip.0, dst_ip.0, &mut tcp_buf)
        .map_err(|_| NetError::BufferTooSmall)?;
    let mut ip_buf = [0u8; 128];
    let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Tcp, 64, &tcp_buf[..tcp_len], &mut ip_buf)
        .map_err(|_| NetError::BufferTooSmall)?;
    let dst_mac = resolve_destination_mac(dst_ip)?;
    let frame = EthernetFrame {
        dst_mac, src_mac: NETWORK_DEVICE.lock().info().mac,
        ethertype: EtherType::Ipv4, payload: &ip_buf[..ip_len],
    };
    let mut eth_buf = [0u8; 1514];
    let len = frame.serialize(&mut eth_buf).map_err(|_| NetError::BufferTooSmall)?;
    transmit_frame(dst_ip, &eth_buf[..len])
}

#[allow(dead_code)]
fn transmit_tcp_payload(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, payload: &[u8]) -> Result<usize, NetError> {
    transmit_tcp_payload_with_window(src_ip, src_port, dst_ip, dst_port, seq, ack, payload, 65535)
}

fn transmit_tcp_payload_with_window(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, payload: &[u8], window: u16) -> Result<usize, NetError> {
    let segment = TcpSegment { src_port, dst_port, seq, ack, flags: 0x18, window, payload };
    let mut tcp_buf = [0u8; 1480];
    let tcp_len = crate::net::tcp::serialize_with_checksum(&segment, src_ip.0, dst_ip.0, &mut tcp_buf).map_err(|_| NetError::BufferTooSmall)?;
    let mut ip_buf = [0u8; 1514];
    let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Tcp, 64, &tcp_buf[..tcp_len], &mut ip_buf).map_err(|_| NetError::BufferTooSmall)?;
    let dst_mac = resolve_destination_mac(dst_ip)?;
    let frame = EthernetFrame { dst_mac, src_mac: NETWORK_DEVICE.lock().info().mac, ethertype: EtherType::Ipv4, payload: &ip_buf[..ip_len] };
    let mut eth_buf = [0u8; 1514];
    let n = frame.serialize(&mut eth_buf).map_err(|_| NetError::BufferTooSmall)?;
    if let Some(task) = crate::task::SCHEDULER.lock().current_task_id() {
        crate::task::process::consume_background_network(task, n)
            .map_err(|_| NetError::QueueFull)?;
    }
    transmit_frame(dst_ip, &eth_buf[..n])?;
    Ok(payload.len())
}

pub fn send_tcp_payload(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, payload: &[u8], caps: &CapabilityTable) -> Result<usize, NetError> {
    send_tcp_payload_with_window(src_ip, src_port, dst_ip, dst_port, seq, ack, payload, 65535, caps)
}

pub fn send_tcp_payload_with_window(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, payload: &[u8], window: u16, caps: &CapabilityTable) -> Result<usize, NetError> {
    if !caps.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE) { return Err(NetError::CapabilityDenied); }
    transmit_tcp_payload_with_window(src_ip, src_port, dst_ip, dst_port, seq, ack, payload, window)
}

#[allow(dead_code)]
fn transmit_tcp_syn(
    src_ip: Ipv4Address,
    src_port: u16,
    dst_ip: Ipv4Address,
    dst_port: u16,
    seq: u32,
) -> Result<(), NetError> {
    transmit_tcp_syn_with_window(src_ip, src_port, dst_ip, dst_port, seq, 65535)
}

fn transmit_tcp_syn_with_window(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, window: u16) -> Result<(), NetError> {
    let segment = TcpSegment {
        src_port, dst_port, seq, ack: 0, flags: 0x02, window, payload: &[],
    };
    let mut tcp_buf = [0u8; 64];
    let tcp_len = crate::net::tcp::serialize_with_checksum(&segment, src_ip.0, dst_ip.0, &mut tcp_buf)
        .map_err(|_| NetError::BufferTooSmall)?;
    let mut ip_buf = [0u8; 128];
    let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Tcp, 64, &tcp_buf[..tcp_len], &mut ip_buf)
        .map_err(|_| NetError::BufferTooSmall)?;
    let dst_mac = resolve_destination_mac(dst_ip)?;
    let frame = EthernetFrame {
        dst_mac, src_mac: NETWORK_DEVICE.lock().info().mac, ethertype: EtherType::Ipv4, payload: &ip_buf[..ip_len],
    };
    let mut eth_buf = [0u8; 1514];
    let len = frame.serialize(&mut eth_buf).map_err(|_| NetError::BufferTooSmall)?;
    transmit_frame(dst_ip, &eth_buf[..len])
}

pub fn send_tcp_syn(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, caps: &CapabilityTable) -> Result<(), NetError> {
    send_tcp_syn_with_window(src_ip, src_port, dst_ip, dst_port, seq, 65535, caps)
}

pub fn send_tcp_syn_with_window(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, window: u16, caps: &CapabilityTable) -> Result<(), NetError> {
    if !caps.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE) { return Err(NetError::CapabilityDenied); }
    transmit_tcp_syn_with_window(src_ip, src_port, dst_ip, dst_port, seq, window)
}

/// Polls the network device and processes all inbound packets.
pub fn poll_network_interface() -> usize {
    let mut processed = 0;
    let _ = poll_tcp_retransmissions(crate::interrupts::get_system_ticks());
    loop {
        let mut rx_buf = [0u8; 1514];
        let len = {
            // Drain the kernel loopback queue first, then the controller's
            // receive ring. Both feed the same demultiplexer below, so
            // 127.0.0.1 traffic is delivered without touching the wire.
            let mut loopback = LOOPBACK_DEVICE.lock();
            match loopback.receive(&mut rx_buf) {
                Ok(len) => len,
                Err(_) => {
                    drop(loopback);
                    let mut device = NETWORK_DEVICE.lock();
                    match device.receive(&mut rx_buf) {
                        Ok(len) => len,
                        Err(e) => {
                            if matches!(e, NetError::QueueFull | NetError::BufferTooSmall) {
                                record_net_drop(NetDropKind::RxRingBackpressure, "rx_ring_backpressure");
                            }
                            break;
                        }
                    }
                }
            }
        };
        processed += 1;
        if let Ok(frame) = EthernetFrame::parse(&rx_buf[..len]) {
            match frame.ethertype {
                EtherType::Arp => {
                    if let Ok(arp_pkt) = ArpPacket::parse(frame.payload) {
                        ARP_TABLE.lock().insert_at(arp_pkt.sender_ip, arp_pkt.sender_mac, crate::interrupts::get_system_ticks());
                        // End-to-end proof: the real ARP request emitted when the
                        // E1000 was bound (emit_gateway_arp_request) left the
                        // guest, and a real reply came back through the
                        // controller's RX ring. Log it once, honestly, with the
                        // reply's real source address, MAC and frame length.
                        if arp_pkt.opcode == ArpOpcode::Reply
                            && arp_pkt.sender_ip == GATEWAY_IP
                            && !LIVE_GATEWAY_ARP_LOGGED.swap(true, Ordering::SeqCst)
                        {
                            serial_println!(
                                "[NET LIVE] Real ARP reply received: gateway {}.{}.{}.{} is at {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}; a real packet left the guest and came back ({} byte Ethernet frame).",
                                GATEWAY_IP.0[0], GATEWAY_IP.0[1], GATEWAY_IP.0[2], GATEWAY_IP.0[3],
                                arp_pkt.sender_mac.0[0], arp_pkt.sender_mac.0[1], arp_pkt.sender_mac.0[2],
                                arp_pkt.sender_mac.0[3], arp_pkt.sender_mac.0[4], arp_pkt.sender_mac.0[5],
                                len
                            );
                        }
                        if let Some(entry) = ARP_REQUESTS.lock().iter_mut().find(|entry| entry.valid && entry.ip == arp_pkt.sender_ip) {
                            entry.valid = false;
                        }
                        if arp_pkt.opcode == ArpOpcode::Request && arp_pkt.target_ip == DEFAULT_HOST_IP {
                            let local_mac = NETWORK_DEVICE.lock().info().mac;
                            let reply = ArpPacket {
                                opcode: ArpOpcode::Reply,
                                sender_mac: local_mac,
                                sender_ip: DEFAULT_HOST_IP,
                                target_mac: arp_pkt.sender_mac,
                                target_ip: arp_pkt.sender_ip,
                            };
                            let mut arp_buf = [0u8; 28];
                            if let Ok(n) = reply.serialize(&mut arp_buf) {
                                let response = EthernetFrame {
                                    dst_mac: arp_pkt.sender_mac,
                                    src_mac: local_mac,
                                    ethertype: EtherType::Arp,
                                    payload: &arp_buf[..n],
                                };
                                let mut frame_buf = [0u8; 1514];
                                if let Ok(n) = response.serialize(&mut frame_buf) {
                                    let _ = NETWORK_DEVICE.lock().transmit(&frame_buf[..n]);
                                }
                            }
                        }
                    }
                }
                EtherType::Ipv4 => {
                    if frame.payload.len() >= 8 && (u16::from_be_bytes([frame.payload[6], frame.payload[7]]) & 0x3FFF != 0) {
                        log_net_drop("ipv4_fragment_discarded_no_reassembly");
                        continue;
                    }
                    if let Ok(ip_pkt) = Ipv4Packet::parse(frame.payload) {
                        if ip_pkt.protocol == IpProtocol::Tcp {
                            if let Ok(segment) = TcpSegment::parse(ip_pkt.payload) {
                                if segment.verify_checksum(ip_pkt.src.0, ip_pkt.dst.0, ip_pkt.payload) {
                                    let conn_id = {
                                        let table = TCP_TABLE.lock();
                                        table.entries().iter().flatten().find(|c| {
                                            c.remote_ip == ip_pkt.src && c.remote_port == segment.src_port && c.local_port == segment.dst_port
                                        }).map(|c| c.id)
                                    };
                                    if let Some(id) = conn_id {
                                        let mut to_ack_syn: Option<(u16, u16, u32, u32, u16)> = None;
                                        let mut to_process_est: Option<(u32, bool, u32)> = None;
                                        let mut need_ack: Option<u32> = None;
                                        {
                                            let mut table = TCP_TABLE.lock();
                                            let _ = table.update_peer_window(id, segment.window);
                                            if let Some(conn) = table.get_mut(id) {
                                                let old_state = conn.state;
                                                if old_state == TcpState::SynSent {
                                                    if valid_syn_ack(&segment, conn.send_seq.wrapping_add(1)) {
                                                        conn.recv_seq = segment.seq.wrapping_add(1);
                                                        conn.send_seq = conn.send_seq.wrapping_add(1);
                                                        conn.snd_una = conn.send_seq;
                                                        conn.state = TcpState::Established;
                                                        conn.syn_pending = false;
                                                        conn.syn_retries = 0;
                                                        serial_println!(
                                                            "[TCP ESTABLISHED] Connection {} established with {}.{}.{}.{}:{} (local port {})",
                                                            id, conn.remote_ip.0[0], conn.remote_ip.0[1], conn.remote_ip.0[2], conn.remote_ip.0[3], conn.remote_port, conn.local_port
                                                        );
                                                        to_ack_syn = Some((conn.local_port, conn.remote_port, conn.send_seq, conn.recv_seq, conn.rcv_wnd.min(u16::MAX as u32) as u16));
                                                    }
                                                } else if old_state == TcpState::Established {
                                                    if valid_established_segment(&segment, conn.recv_seq, conn.snd_una, conn.send_seq) {
                                                        need_ack = Some(segment.ack);
                                                        let fin = segment.flags & 0x01 != 0;
                                                        if !segment.payload.is_empty() || fin {
                                                            to_process_est = Some((conn.id, fin, conn.recv_seq));
                                                        }
                                                    }
                                                } else {
                                                    conn.recv_seq = segment.seq.wrapping_add(segment.payload.len() as u32);
                                                    conn.state = tcp_next_state(old_state, segment.flags);
                                                }
                                            }
                                            if let Some(ack) = need_ack {
                                                let _ = table.acknowledge_tx(id, ack);
                                                let _ = table.acknowledge_fin(id, ack);
                                            }
                                        }
                                        if let Some((ack_local_port, ack_remote_port, ack_seq, ack_recv_seq, ack_window)) = to_ack_syn {
                                            let _ = transmit_tcp_segment_with_window(
                                                ip_pkt.dst,
                                                ack_local_port,
                                                ip_pkt.src,
                                                ack_remote_port,
                                                ack_seq,
                                                ack_recv_seq,
                                                0x10,
                                                ack_window,
                                            );
                                        } else if let Some((c_id, fin, _before)) = to_process_est {
                                            let _accepted = TCP_TABLE.lock().accept_rx_segment(c_id, segment.seq, segment.payload, fin).unwrap_or(0);
                                            let table2 = TCP_TABLE.lock();
                                            if let Some(c) = table2.get(c_id) {
                                                // Always send ACK for valid received segments so peer window advances
                                                let _ = transmit_tcp_segment_with_window(
                                                    ip_pkt.dst, c.local_port, ip_pkt.src, c.remote_port,
                                                    c.send_seq, c.recv_seq, 0x10, c.rcv_wnd.min(u16::MAX as u32) as u16,
                                                );
                                            }
                                        }
                                    } else {
                                        log_net_drop("tcp_no_matching_connection");
                                    }
                                } else {
                                    log_net_drop("tcp_checksum_mismatch");
                                }
                            } else {
                                log_net_drop("tcp_parse_failed");
                            }
                        } else if ip_pkt.protocol == IpProtocol::Udp {
                            if let Ok(udp_dg) = UdpDatagram::parse(ip_pkt.payload) {
                                SOCKET_TABLE.lock().deliver_udp(
                                    udp_dg.dst_port,
                                    ip_pkt.src,
                                    udp_dg.src_port,
                                    udp_dg.payload,
                                );
                            } else {
                                log_net_drop("udp_parse_failed");
                            }
                        } else {
                            record_net_drop(NetDropKind::UnknownL4Protocol, "unsupported_ip_protocol");
                        }
                    } else {
                        log_net_drop("ipv4_parse_failed");
                    }
                }
                _ => {
                    record_net_drop(NetDropKind::NonIpv4Ethertype, "non_ipv4_or_arp_ethertype");
                }
            }
        } else {
            record_net_drop(NetDropKind::MalformedChecksumOrLength, "ethernet_frame_parse_failed");
        }
    }
    processed
}

/// Retransmits one bounded unacknowledged TCP payload when its deadline expires.
pub fn poll_tcp_retransmissions(now: u64) -> usize {
    // Reclaim terminal TCP entries on every bounded network poll. This keeps
    // handshake/retransmission failures from exhausting the fixed table.
    let reaped = {
        let mut table = TCP_TABLE.lock();
        let expired = table.reap_time_wait(crate::interrupts::get_system_ticks());
        expired + table.reap_closed()
    };
    if let Some((_id, remote_ip, local_port, remote_port, seq, window)) = TCP_TABLE.lock().due_syn_retransmit(now) {
        return reaped + if transmit_tcp_syn_with_window(
            if remote_ip.is_loopback() { Ipv4Address::LOCALHOST } else { DEFAULT_HOST_IP },
            local_port, remote_ip, remote_port, seq, window,
        ).is_ok() { 1 } else { 0 };
    }
    let due_fin = TCP_TABLE.lock().due_fin_retransmit(now);
    if let Some((_id, remote_ip, local_port, remote_port, seq, ack, window)) = due_fin {
        return reaped + if transmit_tcp_segment_with_window(if remote_ip.is_loopback() { Ipv4Address::LOCALHOST } else { DEFAULT_HOST_IP }, local_port, remote_ip, remote_port, seq, ack, 0x11, window).is_ok() { 1 } else { 0 };
    }
    let due = TCP_TABLE.lock().due_retransmit(now);
    let Some((id, remote_ip, local_port, remote_port, seq, ack, len)) = due else { return reaped; };
    let window = TCP_TABLE.lock().get(id).map(|c| c.rcv_wnd.min(u16::MAX as u32) as u16).unwrap_or(0);
    let payload = {
        let table = TCP_TABLE.lock();
        let Some(c) = table.get(id) else { return reaped; };
        let mut buf = [0u8; crate::net::tcp::TCP_MAX_SEGMENT];
        buf[..len].copy_from_slice(&c.tx[..len]);
        buf
    };
    reaped + if transmit_tcp_payload_with_window(if remote_ip.is_loopback() { Ipv4Address::LOCALHOST } else { DEFAULT_HOST_IP }, local_port, remote_ip, remote_port, seq, ack, &payload[..len], window).is_ok() { 1 } else { 0 }
}

/// Initializes the network stack and executes comprehensive in-kernel self-tests.
pub fn init() {
    serial_println!("--- [Initializing In-Kernel Network Subsystem] ---");

    // 1. Ethernet Frame Parsing & Serialization Self-Test
    let test_src_mac = MacAddress([0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC]);
    let test_dst_mac = MacAddress([0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54]);
    let test_payload = b"INTER_OS_NET_LAYER";

    let eth_frame = EthernetFrame {
        dst_mac: test_dst_mac,
        src_mac: test_src_mac,
        ethertype: EtherType::Ipv4,
        payload: test_payload,
    };
    let mut raw_eth = [0u8; 64];
    let serialized_len = eth_frame.serialize(&mut raw_eth).expect("Ethernet serialization failed");
    assert_eq!(serialized_len, 14 + test_payload.len());

    let parsed_frame = EthernetFrame::parse(&raw_eth[0..serialized_len]).expect("Ethernet parse failed");
    assert_eq!(parsed_frame.dst_mac, test_dst_mac);
    assert_eq!(parsed_frame.src_mac, test_src_mac);
    assert_eq!(parsed_frame.ethertype, EtherType::Ipv4);
    assert_eq!(parsed_frame.payload, test_payload);
    serial_println!("[TEST] Ethernet II Framing & MAC Layer: Round-trip serialization PASSED.");

    // 2. RFC 791 IPv4 Checksum & Header Verification Self-Test
    let src_ip = Ipv4Address::new(10, 0, 2, 15);
    let dst_ip = Ipv4Address::new(127, 0, 0, 1);
    let mut raw_ip = [0u8; 64];
    let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Udp, 64, b"PAYLOAD", &mut raw_ip)
        .expect("IPv4 serialization failed");

    let parsed_ip = Ipv4Packet::parse(&raw_ip[0..ip_len]).expect("IPv4 parse failed");
    assert_eq!(parsed_ip.src, src_ip);
    assert_eq!(parsed_ip.dst, dst_ip);
    assert_eq!(parsed_ip.protocol, IpProtocol::Udp);
    assert_eq!(parsed_ip.ttl, 64);
    assert_eq!(parsed_ip.payload, b"PAYLOAD");

    // Test tamper detection
    raw_ip[15] ^= 0x01; // Corrupt source IP
    assert!(Ipv4Packet::parse(&raw_ip[0..ip_len]).is_err(), "IPv4 checksum failed to detect corruption!");
    serial_println!("[TEST] RFC 791 IPv4 Checksum Engine & Tamper Detection: PASSED.");

    // 3. ARP Protocol & Cache Self-Test
    let mut arp_table = ARP_TABLE.lock();
    let peer_ip = Ipv4Address::new(10, 0, 2, 2);
    let peer_mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x02]);
    arp_table.insert(peer_ip, peer_mac);
    assert_eq!(arp_table.lookup(peer_ip), Some(peer_mac));
    assert_eq!(arp_table.lookup(Ipv4Address::new(10, 0, 2, 99)), None);
    drop(arp_table);
    serial_println!("[TEST] ARP Resolution & In-Kernel Cache: PASSED.");

    // 4. UDP Datagram Serialization Self-Test
    let mut raw_udp = [0u8; 32];
    let udp_len = UdpDatagram::serialize(8080, 9090, b"DATA", &mut raw_udp).expect("UDP serialization failed");
    let parsed_udp = UdpDatagram::parse(&raw_udp[0..udp_len]).expect("UDP parse failed");
    assert_eq!(parsed_udp.src_port, 8080);
    assert_eq!(parsed_udp.dst_port, 9090);
    assert_eq!(parsed_udp.payload, b"DATA");
    serial_println!("[TEST] UDP Datagram Parsing & Port Multiplexing: PASSED.");

    // 5. Capability-Governed Socket Dispatch & Loopback End-to-End Test
    let mut valid_caps = CapabilityTable::new(8);
    valid_caps.insert(Capability {
        id: CapabilityId(1),
        cap_type: CapabilityType::NetworkSocket,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    valid_caps.insert(Capability {
        id: CapabilityId(2),
        cap_type: CapabilityType::NetworkSend,
        rights: Rights::WRITE,
        object_id: 0,
    }).ok();
    valid_caps.insert(Capability {
        id: CapabilityId(3),
        cap_type: CapabilityType::NetworkReceive,
        rights: Rights::READ,
        object_id: 0,
    }).ok();

    let empty_caps = CapabilityTable::new(8);

    // Create socket with valid capabilities
    let sock_id = SOCKET_TABLE.lock().create_socket(1, 4242, &valid_caps).expect("Socket creation failed");

    // Attempt creation with empty capabilities -> Must fail
    let denied_res = SOCKET_TABLE.lock().create_socket(2, 4243, &empty_caps);
    assert_eq!(denied_res.err(), Some(NetError::CapabilityDenied));
    serial_println!("[TEST] Zero-Trust Socket Creation: Unauthorized creation BLOCKED (Principle #2).");

    // Send UDP packet over loopback
    send_udp_datagram(
        Ipv4Address::LOCALHOST,
        5000,
        Ipv4Address::LOCALHOST,
        4242,
        b"HELLO_INTER_NET",
        &valid_caps,
    ).expect("UDP send failed");

    // Unauthorized send attempt -> Must fail
    let unauth_send = send_udp_datagram(
        Ipv4Address::LOCALHOST,
        5000,
        Ipv4Address::LOCALHOST,
        4242,
        b"EVIL_PACKET",
        &empty_caps,
    );
    assert_eq!(unauth_send.err(), Some(NetError::CapabilityDenied));
    serial_println!("[TEST] Zero-Trust Network Transmit: Unauthorized packet transmission BLOCKED.");

    // Process inbound packets via polling
    let polled = poll_network_interface();
    assert!(polled > 0, "Expected at least 1 packet polled from loopback");

    // Receive datagram through socket
    let mut rx_buf = [0u8; 64];
    let mut sock_table = SOCKET_TABLE.lock();
    let sock = sock_table.get_mut(sock_id).expect("Socket not found");
    let (read_bytes, from_ip, from_port) = sock.recv_from(&mut rx_buf, &valid_caps).expect("Socket recv failed");
    assert_eq!(&rx_buf[0..read_bytes], b"HELLO_INTER_NET");
    assert_eq!(from_ip, Ipv4Address::LOCALHOST);
    assert_eq!(from_port, 5000);

    // Unauthorized recv attempt -> Must fail
    let unauth_recv = sock.recv_from(&mut rx_buf, &empty_caps);
    assert_eq!(unauth_recv.err(), Some(NetError::CapabilityDenied));
    serial_println!("[TEST] Zero-Trust Network Receive: Unauthorized socket read BLOCKED.");

    // Clean up test socket so local port 4242 is freed for userspace netd
    let _ = sock_table.close_owned(sock_id, 1);

    // Mobile Wireless HAL & BLE Beacon Initialization
    wireless::init();

    serial_println!("--- [Network Subsystem 100% Attested & Enforced] ---");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, CapabilityId, CapabilityType, Rights, CapabilityTable, DelegatedLease};
    use crate::capability::delegation::{allocate_lease_id, register_lease};
    use crate::task::TaskId;

    #[test]
    fn test_send_udp_datagram_authorized_quota_policy() {
        let grantor = TaskId(70);
        let grantee = TaskId(71);
        crate::task::process::register_leader(grantee, TaskId(1));

        let mut grantor_caps = CapabilityTable::new_for_task(grantor, 4);
        grantor_caps.insert(Capability {
            id: CapabilityId(301),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE | Rights::GRANT,
            object_id: 0,
        }).unwrap();

        // Create a single-use delegated lease for NetworkSend
        let parent_cap = grantor_caps.get(0).unwrap();
        let lease = DelegatedLease::new(
            allocate_lease_id(),
            parent_cap,
            grantor,
            grantee,
            Rights::WRITE,
            1, // 1 invocation quota
            10_000,
            0,
        ).unwrap();
        let lease_cap = lease.to_capability();
        register_lease(lease);

        let mut grantee_caps = CapabilityTable::new_for_task(grantee, 4);
        grantee_caps.insert(lease_cap).unwrap();

        // 1. When consume_quota is false (e.g. DNS resolver retransmission of already-authorized query),
        // authority must be verified without consuming the single invocation quota!
        let check_res = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST,
            5000,
            Ipv4Address::LOCALHOST,
            4242,
            b"DNS_RETRY",
            &grantee_caps,
            false, // do not consume quota
        );
        assert!(check_res.is_ok(), "Non-consuming datagram authorization must succeed");

        // 2. Now perform an authorized send with consume_quota: true (initial transmission)
        let send_res1 = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST,
            5000,
            Ipv4Address::LOCALHOST,
            4242,
            b"DNS_INITIAL",
            &grantee_caps,
            true, // consume quota
        );
        assert!(send_res1.is_ok(), "First consuming send must succeed");

        // 3. Second consuming send must fail closed because lease quota is exhausted
        let send_res2 = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST,
            5000,
            Ipv4Address::LOCALHOST,
            4242,
            b"DNS_OVER_QUOTA",
            &grantee_caps,
            true, // consume quota
        );
        assert_eq!(send_res2.err(), Some(NetError::CapabilityDenied));

        // 4. Unauthorized capabilities (missing NetworkSend) fail immediately
        let empty_caps = CapabilityTable::new_for_task(grantee, 4);
        let unauth_res = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST,
            5000,
            Ipv4Address::LOCALHOST,
            4242,
            b"ATTACK",
            &empty_caps,
            false,
        );
        assert_eq!(unauth_res.err(), Some(NetError::CapabilityDenied));
    }

    /// Builds a single-invocation `NetworkSend` lease for `grantee` and returns its table.
    fn single_use_send_lease(grantee: TaskId) -> CapabilityTable {
        let grantor = TaskId(grantee.0 + 500);
        let mut grantor_caps = CapabilityTable::new_for_task(grantor, 4);
        grantor_caps.insert(Capability {
            id: CapabilityId(grantor.0 as u64 + 4_000),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE | Rights::GRANT,
            object_id: 0,
        }).unwrap();

        let lease = DelegatedLease::new(
            allocate_lease_id(),
            grantor_caps.get(0).unwrap(),
            grantor,
            grantee,
            Rights::WRITE,
            1,
            10_000,
            0,
        ).unwrap();
        register_lease(lease.clone());

        let mut grantee_caps = CapabilityTable::new_for_task(grantee, 4);
        grantee_caps.insert(lease.to_capability()).unwrap();
        grantee_caps
    }

    fn background_bytes(task: TaskId) -> u32 {
        crate::task::process::record_for_process(crate::task::process::process_for_task(task).unwrap())
            .unwrap()
            .background_network_bytes
    }

    /// WHY this exists: a DNS resolver retransmits an already-authorized logical
    /// query under one delegated lease, so the lease's single invocation is spent
    /// by the initial transmission. The retransmission must still be authorized
    /// on that same logical operation, otherwise the per-logical-operation quota
    /// policy is void and every retry fails closed.
    #[test]
    fn dns_retransmission_is_authorized_after_the_logical_lease_is_spent() {
        let grantee = TaskId(72);
        crate::task::process::register_leader(grantee, TaskId(1));
        let caps = single_use_send_lease(grantee);

        // Initial transmission spends the single invocation.
        let initial = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST, 5000, Ipv4Address::LOCALHOST, 4242,
            b"DNS_INITIAL", &caps, true,
        );
        assert!(initial.is_ok(), "initial transmission must be authorized: {:?}", initial.err());

        // Retransmission of the same in-flight logical query must still be authorized
        // even though the lease now reports zero remaining invocations.
        let retransmit = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST, 5000, Ipv4Address::LOCALHOST, 4242,
            b"DNS_RETRANSMIT", &caps, false,
        );
        assert!(
            retransmit.is_ok(),
            "DNS retransmission of an in-flight logical query must be authorized after the lease quota is spent: {:?}",
            retransmit.err()
        );

        // A retransmission never becomes a general quota bypass: a *new* logical
        // operation (consume_quota = true) is still refused at zero invocations.
        let new_operation = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST, 5000, Ipv4Address::LOCALHOST, 4242,
            b"DNS_SECOND_LOGICAL_OP", &caps, true,
        );
        assert_eq!(
            new_operation.err(),
            Some(NetError::CapabilityDenied),
            "the retransmit path must not become a general bypass of single-use lease enforcement"
        );
    }

    /// A send that capability authorization refuses must not debit the caller's
    /// background network budget. The budget is an allowance for traffic that was
    /// actually put on the wire, so a refused send must leave it untouched.
    ///
    /// This drives `authorize_and_debit_network_send` directly, passing an
    /// explicit caller task, because the budget debit is keyed to the scheduler's
    /// current task and that is `None` outside a scheduled context. Driving the
    /// helper is what makes the ordering observable at all: it is the ordering
    /// this test pins, not the frame serialization around it.
    #[test]
    fn denied_send_does_not_debit_background_network_budget() {
        let grantee = TaskId(73);
        crate::task::process::register_leader(grantee, TaskId(1));
        // Background state is what makes the byte budget actually accounted.
        crate::task::process::set_background(grantee, 0).unwrap();
        let caps = single_use_send_lease(grantee);

        // Denied at the capability gate: no NetworkSend capability at all.
        let empty = CapabilityTable::new_for_task(grantee, 4);
        let denied = authorize_and_debit_network_send(&empty, Some(grantee), 512, false);
        assert_eq!(denied.err(), Some(NetError::CapabilityDenied));
        assert_eq!(
            background_bytes(grantee), 0,
            "a capability-denied send must not debit the background network budget"
        );

        // The one authorized send spends the single lease invocation and debits
        // exactly the frame it accounted for.
        let authorized = authorize_and_debit_network_send(&caps, Some(grantee), 512, true);
        assert!(authorized.is_ok(), "authorized send must be allowed: {:?}", authorized.err());
        assert_eq!(
            background_bytes(grantee), 512,
            "an authorized send must debit exactly the frame length it reported"
        );

        // Denied because the single-use lease is exhausted: still no further debit.
        let spent = authorize_and_debit_network_send(&caps, Some(grantee), 512, true);
        assert_eq!(spent.err(), Some(NetError::CapabilityDenied));
        assert_eq!(
            background_bytes(grantee), 512,
            "an over-quota send must not debit the background network budget"
        );

        // The retransmission of the same in-flight logical query stays authorized
        // after the invocation is spent, and does debit: the bytes go on the wire.
        let retransmit = authorize_and_debit_network_send(&caps, Some(grantee), 256, false);
        assert!(
            retransmit.is_ok(),
            "DNS retransmission must stay authorized after the lease invocation is spent: {:?}",
            retransmit.err()
        );
        assert_eq!(
            background_bytes(grantee), 768,
            "an authorized retransmission must still debit the bytes it actually sent"
        );

        // A budget refusal is reported as QueueFull, distinct from CapabilityDenied,
        // so a caller can tell "no authority" from "no allowance left".
        let exhausted = authorize_and_debit_network_send(&caps, Some(grantee), usize::MAX, false);
        assert_eq!(exhausted.err(), Some(NetError::QueueFull));
    }

    #[test]
    fn test_net_drop_stats_accounting() {
        let prev_non_ip = DROP_STATS.non_ipv4_ethertype.load(Ordering::Relaxed);
        let prev_malformed = DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed);
        let prev_unknown_l4 = DROP_STATS.unknown_l4_protocol.load(Ordering::Relaxed);
        let prev_mismatch = DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed);
        let prev_backpressure = DROP_STATS.rx_backpressure.load(Ordering::Relaxed);
        let prev_fragment = DROP_STATS.ipv4_fragment.load(Ordering::Relaxed);

        record_net_drop(NetDropKind::NonIpv4Ethertype, "test_lldp");
        record_net_drop(NetDropKind::MalformedChecksumOrLength, "test_cksum");
        record_net_drop(NetDropKind::UnknownL4Protocol, "test_gre");
        record_net_drop(NetDropKind::PortOrConnectionMismatch, "test_port");
        record_net_drop(NetDropKind::RxRingBackpressure, "test_bp");
        record_net_drop(NetDropKind::Ipv4Fragment, "test_frag");

        assert_eq!(DROP_STATS.non_ipv4_ethertype.load(Ordering::Relaxed), prev_non_ip + 1);
        assert_eq!(DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed), prev_malformed + 1);
        assert_eq!(DROP_STATS.unknown_l4_protocol.load(Ordering::Relaxed), prev_unknown_l4 + 1);
        assert_eq!(DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed), prev_mismatch + 1);
        assert_eq!(DROP_STATS.rx_backpressure.load(Ordering::Relaxed), prev_backpressure + 1);
        assert_eq!(DROP_STATS.ipv4_fragment.load(Ordering::Relaxed), prev_fragment + 1);
    }
}

