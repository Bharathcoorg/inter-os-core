// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
pub use tcp::{is_closing_state as tcp_is_closing_state, next_state as tcp_next_state, rst_action as tcp_rst_action, seq_ge, seq_gt, seq_le, seq_lt, valid_closing_segment, valid_established_segment, valid_syn_ack, RstAction, TcpSegment, TcpState};
use crate::task::TaskId;
pub use tls::{ContentType as TlsContentType, TlsRecord};
pub use ethernet::{EtherType, EthernetFrame, MacAddress};
pub use ipv4::{calculate_ipv4_checksum, verify_ipv4_checksum, IpProtocol, Ipv4Address, Ipv4Packet};
pub use socket::{SocketId, SocketTable, TcpConnection, TcpConnectionTable, UdpSocket, TCP_MAX_RETRIES, TCP_RETRANSMIT_TICKS};
pub use udp::{verify_udp_checksum, UdpDatagram};
pub use wireless::{WifiState, WirelessManager, WIRELESS_MANAGER};
pub use route::{Ipv4Cidr, Route, RoutingTable, MAX_ROUTES};
pub use ipv6::{
    build_dad_probe_frame, calculate_icmpv6_checksum, verify_icmpv6_checksum, verify_ndp_hop_limit,
    Icmpv6Neighbor, Icmpv6RouterAdvertisement, Ipv6Address, Ipv6Autoconfig,
    Ipv6Packet, Ipv6Prefix, Ipv6Route, Ipv6RoutingTable, NeighborCache, NeighborEntry,
};

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

    #[cfg(test)]
    pub fn net_drop_stats_reset(&self) {
        self.non_ipv4_ethertype.store(0, Ordering::Relaxed);
        self.malformed_checksum_length.store(0, Ordering::Relaxed);
        self.unknown_l4_protocol.store(0, Ordering::Relaxed);
        self.port_conn_mismatch.store(0, Ordering::Relaxed);
        self.rx_backpressure.store(0, Ordering::Relaxed);
        self.ipv4_fragment.store(0, Ordering::Relaxed);
        self.logged_mask.store(0, Ordering::Relaxed);
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
    crate::serial_println!("[NET DROP] {} ({:?})", detail, kind);
    DROP_STATS.record_drop(kind, detail);
}

pub fn log_net_drop(reason: &str) {
    let kind = match reason {
        "ipv4_fragment_discarded_no_reassembly" => NetDropKind::Ipv4Fragment,
        "tcp_checksum_mismatch" | "tcp_parse_failed" | "udp_parse_failed" | "ipv4_parse_failed" => NetDropKind::MalformedChecksumOrLength,
        "tcp_no_matching_connection" | "tcp_reset_out_of_window" | "tcp_closing_segment_out_of_window" => NetDropKind::PortOrConnectionMismatch,
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
pub const LOOPBACK_MAC: MacAddress = MacAddress([0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
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

pub const ARP_REQUEST_TIMEOUT_TICKS: u64 = 20_000;

fn record_in_flight_arp_request(ip: Ipv4Address, now: u64) {
    let mut requests = ARP_REQUESTS.lock();
    let slot_index = if let Some(idx) = requests.iter().position(|entry| !entry.valid || entry.ip == ip) {
        idx
    } else {
        requests.iter().enumerate().min_by_key(|(_, e)| e.requested_at).map(|(i, _)| i).unwrap_or(0)
    };
    requests[slot_index] = ArpRequestState { ip, requested_at: now, valid: true };
}

fn take_in_flight_arp_request(ip: Ipv4Address, now: u64) -> bool {
    let mut requests = ARP_REQUESTS.lock();
    if let Some(entry) = requests.iter_mut().find(|entry| entry.valid && entry.ip == ip) {
        if now.saturating_sub(entry.requested_at) <= ARP_REQUEST_TIMEOUT_TICKS {
            entry.valid = false;
            true
        } else {
            entry.valid = false;
            false
        }
    } else {
        false
    }
}

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

/// Global IPv6 Neighbor Discovery Protocol (NDP) resolution cache.
pub static IPV6_NEIGHBOR_CACHE: Mutex<crate::net::ipv6::NeighborCache> = Mutex::new(crate::net::ipv6::NeighborCache::new());

/// Global IPv6 routing table.
pub static IPV6_ROUTING_TABLE: Mutex<crate::net::ipv6::Ipv6RoutingTable> = Mutex::new(crate::net::ipv6::Ipv6RoutingTable::new());

/// Global IPv6 address lifecycle table (SLAAC / DAD).
pub static IPV6_ADDRESSES: Mutex<crate::net::ipv6::Ipv6AddressTable> = Mutex::new(crate::net::ipv6::Ipv6AddressTable::new());

/// Host link-local IPv6 address (e.g. fe80::... derived from MAC or SLAAC).
pub static HOST_IPV6_LINK_LOCAL: Mutex<crate::net::ipv6::Ipv6Address> = Mutex::new(crate::net::ipv6::Ipv6Address::UNSPECIFIED);

/// One word from the kernel's real entropy source, or a refusal.
///
/// WHY this exists (M1, M6): the DNS transaction id was computed from
/// `get_system_ticks()` and the TCP initial sequence number was whatever the
/// caller handed in, so an off-path observer could derive both. The source is
/// `crypto::rng::TRNG`. `HardwareRng::init` reads CPUID leaf 1 ECX bit 30 before
/// it will execute RDRAND at all (`crypto/rng.rs:106`) and retries the
/// instruction ten times per draw (`crypto/rng.rs:255`), so on the x86_64 guest
/// -- whose CPU model is `qemu64,+rdrand` -- a word is 64 bits of hardware
/// randomness. This is not a tick-seeded mixer and is not described as one.
///
/// The honest bound per consumer: DNS takes 16 bits of the word (the wire id
/// field), the resolver source port takes at most 3 bits because only eight
/// ports are reserved for it, and the TCP ISN takes 32 bits.
///
/// WHY fail-closed: `try_next_u64` refuses when the CPU has no RDRAND and no
/// firmware seed was injected (`DegradedPseudoOnly`), and when the pool was
/// never initialized. A caller that cannot get a word must refuse to start the
/// query or open the connection; falling back to ticks would restore exactly
/// the predictability this removes.
///
/// WHY `try_lock` and not `lock`: parts of the receive path run in interrupt
/// context on this single-CPU guest, where blocking on a mutex the interrupted
/// code already holds can never be satisfied. Contention is a refusal, not a
/// wait, so this adds no blocking lock to any network path.
pub(crate) fn net_entropy_u64() -> Result<u64, crate::crypto::EntropyError> {
    let mut rng = crate::crypto::TRNG
        .try_lock()
        .ok_or(crate::crypto::EntropyError::HardwareUnavailable)?;
    if rng.state == crate::crypto::EntropyState::Uninitialized {
        // Boot initializes the TRNG (`main.rs`), so an uninitialized pool in a
        // guest is a refusal. A unit test has no boot sequence; it gets the same
        // deterministic fixture `HardwareRng::init` installs under `cfg(test)`.
        #[cfg(test)]
        {
            rng.inject_test_seed(&[0x5A; 32]);
        }
        #[cfg(not(test))]
        {
            return Err(crate::crypto::EntropyError::InsufficientEntropy);
        }
    }
    rng.try_next_u64()
}

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
    let res = NETWORK_DEVICE.lock().transmit(&frame_buf[..len]);
    if res.is_ok() {
        let now = crate::interrupts::get_system_ticks();
        record_in_flight_arp_request(target_ip, now);
    }
    res
}

/// Whether one address is reachable without a router, on the prefix this host
/// itself is configured on.
fn is_on_local_link(ip: Ipv4Address) -> bool {
    ip.0[..3] == DEFAULT_HOST_IP.0[..3]
}

/// The address a packet is handed to on the wire, which is the peer itself on
/// the local link and the router for everything else.
fn next_hop_for(dst_ip: Ipv4Address) -> Ipv4Address {
    if is_on_local_link(dst_ip) { dst_ip } else { GATEWAY_IP }
}

/// Resolves the Ethernet address a packet is handed to, arming a background ARP
/// request when that address is not yet known.
///
/// WHY the router answers for addresses outside this host's own prefix: QEMU's
/// user-mode network replies to ARP only for its gateway, so a `who-has` for a
/// public address can never be answered. While the destination address itself
/// was the only thing asked about, every off-link packet died in this function
/// and no SYN to any host beyond the virtual router ever left the kernel. The
/// IP header still names the real peer; only the Ethernet address is the
/// router's.
fn resolve_destination_mac(dst_ip: Ipv4Address) -> Result<MacAddress, NetError> {
    if dst_ip.is_loopback() {
        return Ok(LOOPBACK_MAC);
    }
    let next_hop = next_hop_for(dst_ip);
    let now = crate::interrupts::get_system_ticks();
    if let Some(mac) = ARP_TABLE.lock().lookup_at(next_hop, now) {
        return Ok(mac);
    }
    // Fall back to existing cached mapping while triggering background refresh
    if let Some(mac) = ARP_TABLE.lock().lookup(next_hop) {
        let _ = transmit_arp_request(next_hop);
        return Ok(mac);
    }
    let mut requests = ARP_REQUESTS.lock();
    if let Some(entry) = requests.iter().find(|entry| entry.valid && entry.ip == next_hop) {
        if now.saturating_sub(entry.requested_at) < ARP_REQUEST_RETRY_TICKS {
            return Err(NetError::NoPacketAvailable);
        }
    }
    let slot_index = requests.iter().position(|entry| !entry.valid || entry.ip == next_hop).unwrap_or(0);
    requests[slot_index] = ArpRequestState { ip: next_hop, requested_at: now, valid: true };
    drop(requests);
    transmit_arp_request(next_hop)?;
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
        Ok(()) => {
            let now = crate::interrupts::get_system_ticks();
            record_in_flight_arp_request(GATEWAY_IP, now);
            serial_println!(
                "[NET LIVE] Real ARP who-has {}.{}.{}.{} handed to the {:?} NIC as a {} byte Ethernet frame; awaiting gateway reply.",
                GATEWAY_IP.0[0], GATEWAY_IP.0[1], GATEWAY_IP.0[2], GATEWAY_IP.0[3],
                info.kind, frame_len
            );
        }
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
/// `caller_task` comes from [`caller_task_id`], which is the scheduler's current
/// task; it is `None` outside a scheduled context, where no process budget
/// exists to debit.
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

/// The task whose background-network allowance the next send is billed to.
///
/// WHY the scheduler's current task and not the capability table's owner: the
/// budget is a per-process resource limit, and the process that puts bytes on the
/// wire is the one executing, whatever table it happens to be holding. A caller
/// outside any scheduled context (`None`) has no process to bill, which is why the
/// debit is skipped rather than failed.
///
/// WHY a function instead of the inline read at both call sites: the host unit-test
/// process has no scheduled context it can hand a case, so a test-only override sits
/// behind `#[cfg(test)]` here and compiles out of the guest binary entirely. See
/// `SendCallerGuard`.
fn caller_task_id() -> Option<TaskId> {
    #[cfg(test)]
    if let Some(task) = *TEST_SEND_CALLER.lock() {
        return Some(task);
    }
    crate::task::SCHEDULER.lock().current_task_id()
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
    let caller_task = caller_task_id();
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
    if let Some(task) = caller_task_id() {
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
    // WHY the sequence on the wire is re-read from the connection (M6): the ISN
    // is now drawn by the kernel inside `TcpConnectionTable::open`, but the
    // connect syscall still forwards the value its caller supplied. Putting that
    // value on the wire while the table holds a different one would desynchronise
    // the handshake from itself -- `valid_syn_ack` compares against
    // `conn.send_seq + 1`, so the connection could never establish. The table is
    // the authority; the argument is only a fallback.
    //
    // WHY `try_lock`: `poll_tcp_retransmissions` calls this from an `if let`
    // scrutinee whose temporary `TCP_TABLE` guard lives for the whole body, and
    // the mutex is not reentrant. When that guard is held the argument is already
    // `conn.syn_seq`, so declining to re-read it is correct, not a compromise.
    let seq = match TCP_TABLE.try_lock() {
        Some(table) => table.entries().iter().flatten()
            .find(|c| c.syn_pending
                && c.local_ip == src_ip
                && c.local_port == src_port
                && c.remote_ip == dst_ip
                && c.remote_port == dst_port)
            .map(|c| c.syn_seq)
            .unwrap_or(seq),
        None => seq,
    };
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
    const MAX_POLL_BUDGET: usize = 32;
    let mut processed = 0;
    let _ = poll_tcp_retransmissions(crate::interrupts::get_system_ticks());
    while processed < MAX_POLL_BUDGET {
        let mut rx_buf = [0u8; 1514];
        let (len, is_loopback) = {
            // Drain the kernel loopback queue first, then the controller's
            // receive ring. Both feed the same demultiplexer below, so
            // 127.0.0.1 traffic is delivered without touching the wire.
            let mut loopback = LOOPBACK_DEVICE.lock();
            match loopback.receive(&mut rx_buf) {
                Ok(len) => (len, true),
                Err(_) => {
                    drop(loopback);
                    let mut device = NETWORK_DEVICE.lock();
                    match device.receive(&mut rx_buf) {
                        Ok(len) => (len, false),
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
            let local_mac = if is_loopback {
                LOOPBACK_MAC
            } else {
                NETWORK_DEVICE.lock().info().mac
            };

            // Universal Ethernet L2 destination filtering: must be unicast to local station, broadcast, or multicast
            if !frame.dst_mac.is_broadcast() && !frame.dst_mac.is_multicast() && frame.dst_mac != local_mac {
                record_net_drop(NetDropKind::PortOrConnectionMismatch, "ethernet_dst_mac_mismatch");
                continue;
            }

            match frame.ethertype {
                EtherType::Arp => {
                    if let Ok(arp_pkt) = ArpPacket::parse(frame.payload) {
                        // Cross-layer validation: sender MAC in ARP payload MUST match Ethernet source MAC
                        if frame.src_mac != arp_pkt.sender_mac {
                            record_net_drop(NetDropKind::MalformedChecksumOrLength, "arp_mac_inconsistency");
                            continue;
                        }

                        // Anti-spoofing: sender IP must not claim our host IP
                        if arp_pkt.sender_ip == DEFAULT_HOST_IP {
                            record_net_drop(NetDropKind::MalformedChecksumOrLength, "arp_self_ip_spoof");
                            continue;
                        }

                        match arp_pkt.opcode {
                            ArpOpcode::Reply => {
                                // RFC 826: ARP replies must be unicast to our local MAC (never broadcast/multicast)
                                if frame.dst_mac.is_broadcast() || frame.dst_mac.is_multicast() {
                                    record_net_drop(NetDropKind::MalformedChecksumOrLength, "arp_broadcast_reply_drop");
                                    continue;
                                }

                                // Validate target MAC and IP in ARP Reply
                                if arp_pkt.target_mac != local_mac || arp_pkt.target_ip != DEFAULT_HOST_IP {
                                    record_net_drop(NetDropKind::MalformedChecksumOrLength, "arp_reply_target_mismatch");
                                    continue;
                                }

                                // Strictly require in-flight correlation; reject unsolicited replies
                                let now = crate::interrupts::get_system_ticks();
                                let had_inflight_request = take_in_flight_arp_request(arp_pkt.sender_ip, now);
                                if !had_inflight_request {
                                    record_net_drop(NetDropKind::PortOrConnectionMismatch, "arp_unsolicited_reply_dropped");
                                    continue;
                                }

                                // WHY the gateway is no longer pinned permanently (M2): the
                                // first reply to a correlated request was written with
                                // `insert_permanent`, and `arp.rs` refuses to overwrite a
                                // permanent entry, so whichever responder won that one race
                                // owned the L2 next hop for the whole uptime -- a persistent
                                // MITM, not a transient one. The mapping now ages on the same
                                // bounded lifetime as every other neighbour (`ARP_CACHE_TICKS`)
                                // and a later correlated reply replaces it.
                                //
                                // The residual race, stated plainly: in-flight correlation
                                // proves only that *we* asked, not who answered. An attacker
                                // who replies before the real gateway still wins that round,
                                // and `resolve_destination_mac` keeps serving the cached MAC
                                // while it revalidates. What changed is the bound -- one cache
                                // lifetime instead of forever -- and replaceability.
                                let now = crate::interrupts::get_system_ticks();
                                ARP_TABLE.lock().insert_at(arp_pkt.sender_ip, arp_pkt.sender_mac, now);

                                if arp_pkt.sender_ip == GATEWAY_IP
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
                            }
                            ArpOpcode::Request => {
                                if arp_pkt.target_ip == DEFAULT_HOST_IP {
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

                                    // Only refresh existing dynamic mappings; never allocate new slots or touch GATEWAY_IP
                                    if arp_pkt.sender_ip != GATEWAY_IP {
                                        let now = crate::interrupts::get_system_ticks();
                                        let mut table = ARP_TABLE.lock();
                                        if table.lookup(arp_pkt.sender_ip).is_some() && !table.is_permanent(arp_pkt.sender_ip) {
                                            table.insert_at(arp_pkt.sender_ip, arp_pkt.sender_mac, now);
                                        }
                                    }
                                }
                            }
                            ArpOpcode::Unknown(_) => {
                                record_net_drop(NetDropKind::MalformedChecksumOrLength, "arp_unknown_opcode");
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
                        if ip_pkt.dst != DEFAULT_HOST_IP && ip_pkt.dst != Ipv4Address::BROADCAST && !ip_pkt.dst.is_loopback() {
                            log_net_drop("ipv4_destination_mismatch");
                            continue;
                        }
                        if ip_pkt.protocol == IpProtocol::Tcp {
                            if let Ok(segment) = TcpSegment::parse(ip_pkt.payload) {
                                if segment.verify_checksum(ip_pkt.src.0, ip_pkt.dst.0, ip_pkt.payload) {
                                    let conn_id = {
                                        let table = TCP_TABLE.lock();
                                        table.entries().iter().flatten().find(|c| {
                                            c.remote_ip == ip_pkt.src && c.remote_port == segment.src_port && c.local_port == segment.dst_port && c.local_ip == ip_pkt.dst
                                        }).map(|c| c.id)
                                    };
                                    if let Some(id) = conn_id {
                                        let mut to_ack_syn: Option<(u16, u16, u32, u32, u16)> = None;
                                        let mut to_process_est: Option<(u32, bool, u32)> = None;
                                        let mut need_ack: Option<u32> = None;
                                        let mut closing_ack: Option<u32> = None;
                                        let mut reset_by_peer = false;
                                        let mut to_challenge: Option<(u16, u16, u32, u32, u16)> = None;
                                        {
                                            let mut table = TCP_TABLE.lock();
                                            // The advertised window is only believed once the segment
                                            // carrying it has been accepted. Updating it up front let a
                                            // forged segment -- including a reset we were about to drop --
                                            // set snd_wnd to zero and stall the connection for good.
                                            let mut segment_accepted = false;
                                            if let Some(conn) = table.get_mut(id) {
                                                let old_state = conn.state;
                                                if segment.flags & 0x04 != 0 {
                                                    // In SYN-SENT the SYN is on the wire but send_seq
                                                    // has not advanced past it yet, so snd_nxt is one
                                                    // beyond send_seq there and equal to it elsewhere.
                                                    let snd_nxt = if old_state == TcpState::SynSent {
                                                        conn.send_seq.wrapping_add(1)
                                                    } else {
                                                        conn.send_seq
                                                    };
                                                    match tcp_rst_action(old_state, segment.seq, segment.ack, conn.recv_seq, snd_nxt, conn.rcv_wnd) {
                                                        RstAction::Accept => reset_by_peer = true,
                                                        RstAction::ChallengeAck => {
                                                            to_challenge = Some((conn.local_port, conn.remote_port, conn.send_seq, conn.recv_seq, conn.rcv_wnd.min(u16::MAX as u32) as u16));
                                                        }
                                                        RstAction::Drop => log_net_drop("tcp_reset_out_of_window"),
                                                    }
                                                } else if old_state == TcpState::SynSent {
                                                    if valid_syn_ack(&segment, conn.send_seq.wrapping_add(1)) {
                                                        segment_accepted = true;
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
                                                        segment_accepted = true;
                                                        need_ack = Some(segment.ack);
                                                        let fin = segment.flags & 0x01 != 0;
                                                        if !segment.payload.is_empty() || fin {
                                                            to_process_est = Some((conn.id, fin, conn.recv_seq));
                                                        }
                                                    }
                                                } else {
                                                    // M4: the closing states used to believe any segment,
                                                    // overwriting recv_seq from the wire and handing an
                                                    // unvalidated ack to acknowledge_fin. SYN-RECEIVED has
                                                    // no established receive window to validate against.
                                                    let acceptable = !tcp_is_closing_state(old_state)
                                                        || valid_closing_segment(&segment, conn.recv_seq, conn.rcv_wnd, conn.snd_una, conn.send_seq);
                                                    if !acceptable {
                                                        log_net_drop("tcp_closing_segment_out_of_window");
                                                    } else {
                                                        segment_accepted = true;
                                                        conn.recv_seq = segment.seq.wrapping_add(segment.payload.len() as u32);
                                                        if conn.fin_pending {
                                                            // A FIN we sent is still unacknowledged, so the
                                                            // state only moves once this segment's ACK has
                                                            // been recorded: acknowledge_fin is what stamps
                                                            // the TIME-WAIT hold. Letting the generic table
                                                            // jump to Closed first leaves the retry timer
                                                            // armed against a slot nobody will free.
                                                            closing_ack = Some(segment.ack);
                                                        } else {
                                                            conn.state = tcp_next_state(old_state, segment.flags);
                                                        }
                                                    }
                                                }
                                            }
                                            if segment_accepted {
                                                let _ = table.update_peer_window(id, segment.window);
                                            }
                                            if let Some(ack) = need_ack {
                                                let _ = table.acknowledge_tx(id, ack);
                                                let _ = table.acknowledge_fin(id, ack);
                                            }
                                            if reset_by_peer {
                                                let _ = table.abort_by_peer(id);
                                            } else if let Some(ack) = closing_ack {
                                                let _ = table.acknowledge_fin(id, ack);
                                            }
                                        }
                                        if let Some((ch_local_port, ch_remote_port, ch_seq, ch_recv_seq, ch_window)) = to_challenge {
                                            // RFC 5961 challenge ACK: our own sequence numbers, no reset
                                            // and no state change. A peer that genuinely lost state answers
                                            // with a reset at the sequence we just told it; a forger cannot.
                                            let _ = transmit_tcp_segment_with_window(
                                                ip_pkt.dst, ch_local_port, ip_pkt.src, ch_remote_port,
                                                ch_seq, ch_recv_seq, 0x10, ch_window,
                                            );
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
                                if !verify_udp_checksum(ip_pkt.src.0, ip_pkt.dst.0, ip_pkt.payload) {
                                    log_net_drop("udp_checksum_mismatch");
                                    continue;
                                }
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
                EtherType::Ipv6 => {
                    if let Ok(ip6_pkt) = Ipv6Packet::parse(frame.payload) {
                        // Drop packets with multicast source address (RFC 4291 § 2.7)
                        if ip6_pkt.src.is_multicast() {
                            record_net_drop(NetDropKind::MalformedChecksumOrLength, "ipv6_multicast_source");
                            continue;
                        }

                        if ip6_pkt.next_header == 58 { // ICMPv6
                            if ip6_pkt.payload.len() >= 4 {
                                // RFC 4443 & RFC 8200: Pseudo-header checksum MUST be validated before ANY state mutation
                                if !verify_icmpv6_checksum(&ip6_pkt.src, &ip6_pkt.dst, ip6_pkt.payload) {
                                    record_net_drop(NetDropKind::MalformedChecksumOrLength, "icmpv6_checksum_mismatch");
                                    continue;
                                }

                                let icmp_type = ip6_pkt.payload[0];
                                let icmp_code = ip6_pkt.payload[1];

                                // RFC 4861: NDP messages (133..=137) MUST have Hop Limit == 255 and Code == 0
                                if (133..=137).contains(&icmp_type) {
                                    if !verify_ndp_hop_limit(ip6_pkt.hop_limit) {
                                        record_net_drop(NetDropKind::MalformedChecksumOrLength, "ndp_hop_limit_not_255");
                                        continue;
                                    }
                                    if icmp_code != 0 {
                                        record_net_drop(NetDropKind::MalformedChecksumOrLength, "ndp_code_not_zero");
                                        continue;
                                    }
                                }

                                match icmp_type {
                                    135 => { // Neighbor Solicitation
                                        if let Ok(parsed) = Icmpv6Neighbor::parse(ip6_pkt.payload) {
                                            // RFC 4861 § 7.1.1: If IP source is unspecified (::), packet is DAD probe
                                            if ip6_pkt.src.is_unspecified() {
                                                // DAD probe MUST NOT contain SLLA option
                                                if parsed.link_layer_address.is_some() {
                                                    record_net_drop(NetDropKind::MalformedChecksumOrLength, "dad_ns_has_slla");
                                                    continue;
                                                }

                                                // RFC 4862 DAD conflict detection:
                                                // If target matches our tentative address, DAD failed!
                                                let mut addrs = IPV6_ADDRESSES.lock();
                                                if addrs.contains_tentative(parsed.target) {
                                                    addrs.mark_invalid(parsed.target);
                                                    let mut host_ip = HOST_IPV6_LINK_LOCAL.lock();
                                                    if *host_ip == parsed.target {
                                                        *host_ip = Ipv6Address::UNSPECIFIED;
                                                    }
                                                    drop(host_ip);
                                                    drop(addrs);
                                                    continue;
                                                }
                                                let is_assigned = addrs.contains_assigned(parsed.target);
                                                drop(addrs);

                                                // If target matches an established assigned address, defend it!
                                                // RFC 4861 § 7.2.4: When responding to a DAD probe (src == ::),
                                                // NA MUST be addressed to all-nodes multicast (ff02::1),
                                                // Solicited flag MUST be 0, Override flag MUST be 1.
                                                if is_assigned {
                                                    let local_mac = NETWORK_DEVICE.lock().info().mac;
                                                    let mut na_buf = [0u8; 32];
                                                    na_buf[0] = 136; // NA
                                                    na_buf[1] = 0;
                                                    na_buf[2..4].fill(0);
                                                    na_buf[4] = 0x20; // Solicited=0, Override=1 (RFC 4861 § 7.2.4)
                                                    na_buf[8..24].copy_from_slice(&parsed.target.0);
                                                    na_buf[24] = 2; // TLLA
                                                    na_buf[25] = 1; // 8 bytes
                                                    na_buf[26..32].copy_from_slice(&local_mac.0);

                                                    let na_dst_ip = Ipv6Address::ALL_NODES;
                                                    let na_csum = calculate_icmpv6_checksum(&parsed.target, &na_dst_ip, &na_buf);
                                                    na_buf[2..4].copy_from_slice(&na_csum.to_be_bytes());

                                                    let ip6_resp = Ipv6Packet {
                                                        src: parsed.target,
                                                        dst: na_dst_ip,
                                                        next_header: 58,
                                                        hop_limit: 255,
                                                        payload: &na_buf,
                                                    };
                                                    let mut ip6_buf = [0u8; 128];
                                                    if let Ok(n_ip6) = ip6_resp.serialize(&mut ip6_buf) {
                                                        let eth_resp = EthernetFrame {
                                                            dst_mac: MacAddress([0x33, 0x33, 0x00, 0x00, 0x00, 0x01]),
                                                            src_mac: local_mac,
                                                            ethertype: EtherType::Ipv6,
                                                            payload: &ip6_buf[..n_ip6],
                                                        };
                                                        let mut eth_buf = [0u8; 1514];
                                                        if let Ok(n_eth) = eth_resp.serialize(&mut eth_buf) {
                                                            let _ = NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]);
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Unicast Neighbor Solicitation
                                                if let Some(slla_mac) = parsed.link_layer_address {
                                                    // RFC 4861: Cross-layer MAC validation
                                                    if slla_mac != frame.src_mac.0 {
                                                        record_net_drop(NetDropKind::MalformedChecksumOrLength, "ndp_slla_mac_mismatch");
                                                        continue;
                                                    }
                                                    let now = crate::interrupts::get_system_ticks();
                                                    IPV6_NEIGHBOR_CACHE.lock().learn(ip6_pkt.src, slla_mac, now, 300_000);
                                                }

                                                let host_ip = *HOST_IPV6_LINK_LOCAL.lock();
                                                // Only reply if host IP is established and matches target
                                                if !host_ip.is_unspecified() && parsed.target == host_ip {
                                                    let local_mac = NETWORK_DEVICE.lock().info().mac;
                                                    let mut na_buf = [0u8; 32];
                                                    na_buf[0] = 136; // NA
                                                    na_buf[1] = 0;
                                                    na_buf[2..4].fill(0);
                                                    na_buf[4] = 0x60; // Solicited=1, Override=1
                                                    na_buf[8..24].copy_from_slice(&parsed.target.0);
                                                    na_buf[24] = 2; // TLLA
                                                    na_buf[25] = 1; // 8 bytes
                                                    na_buf[26..32].copy_from_slice(&local_mac.0);

                                                    let na_csum = calculate_icmpv6_checksum(&host_ip, &ip6_pkt.src, &na_buf);
                                                    na_buf[2..4].copy_from_slice(&na_csum.to_be_bytes());

                                                    let ip6_resp = Ipv6Packet {
                                                        src: host_ip,
                                                        dst: ip6_pkt.src,
                                                        next_header: 58,
                                                        hop_limit: 255,
                                                        payload: &na_buf,
                                                    };
                                                    let mut ip6_buf = [0u8; 128];
                                                    if let Ok(n_ip6) = ip6_resp.serialize(&mut ip6_buf) {
                                                        let eth_resp = EthernetFrame {
                                                            dst_mac: frame.src_mac,
                                                            src_mac: local_mac,
                                                            ethertype: EtherType::Ipv6,
                                                            payload: &ip6_buf[..n_ip6],
                                                        };
                                                        let mut eth_buf = [0u8; 1514];
                                                        if let Ok(n_eth) = eth_resp.serialize(&mut eth_buf) {
                                                            let _ = NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    136 => { // Neighbor Advertisement
                                        if let Ok(parsed) = Icmpv6Neighbor::parse(ip6_pkt.payload) {
                                            // RFC 4861 § 7.2.5: Silently discard any NA with Solicited=1 and multicast destination
                                            if ip6_pkt.dst.is_multicast() && parsed.solicited {
                                                record_net_drop(NetDropKind::MalformedChecksumOrLength, "solicited_na_to_multicast");
                                                continue;
                                            }

                                            // RFC 4862: If target matches our tentative address, DAD failed!
                                            let mut addrs = IPV6_ADDRESSES.lock();
                                            if addrs.contains_tentative(parsed.target) {
                                                addrs.mark_invalid(parsed.target);
                                                drop(addrs);
                                                continue;
                                            }
                                            drop(addrs);

                                            if let Some(tlla_mac) = parsed.link_layer_address {
                                                // Cross-layer MAC validation if unicast source
                                                if !ip6_pkt.src.is_unspecified() && tlla_mac != frame.src_mac.0 {
                                                    record_net_drop(NetDropKind::MalformedChecksumOrLength, "ndp_tlla_mac_mismatch");
                                                    continue;
                                                }
                                                let now = crate::interrupts::get_system_ticks();
                                                let _ = IPV6_NEIGHBOR_CACHE.lock().learn_advertisement_with_router(
                                                    parsed.target,
                                                    tlla_mac,
                                                    parsed.router,
                                                    parsed.override_flag,
                                                    now,
                                                    300_000,
                                                );
                                            }
                                        }
                                    }
                                    134 => { // Router Advertisement
                                        if !ip6_pkt.src.is_link_local() {
                                            record_net_drop(NetDropKind::MalformedChecksumOrLength, "ra_source_not_link_local");
                                            continue;
                                        }
                                        if let Ok(ra) = Icmpv6RouterAdvertisement::parse(ip6_pkt.payload) {
                                            let now = crate::interrupts::get_system_ticks();
                                            // Learn router in neighbor cache
                                            let _ = IPV6_NEIGHBOR_CACHE.lock().learn_advertisement_with_router(
                                                ip6_pkt.src,
                                                frame.src_mac.0,
                                                true,
                                                true,
                                                now,
                                                300_000,
                                            );

                                            // RFC 4861 § 6.3.4: Router Lifetime 0 means router must NOT be default router
                                            if ra.router_lifetime == 0 {
                                                IPV6_ROUTING_TABLE.lock().remove_next_hop(&ip6_pkt.src);
                                            } else {
                                                let _ = IPV6_ROUTING_TABLE.lock().add(Ipv6Route {
                                                    prefix: Ipv6Prefix::new(Ipv6Address::UNSPECIFIED, 0),
                                                    next_hop: Some(ip6_pkt.src),
                                                    interface: 0,
                                                    metric: 10,
                                                });
                                            }

                                            // Process Prefix Information Options (PIO) per RFC 4862 SLAAC
                                            let local_mac = NETWORK_DEVICE.lock().info().mac;
                                            if let Some(pio) = ra.prefix_info {
                                                if pio.valid_lifetime == 0 {
                                                    // Prefix withdrawal
                                                    IPV6_ADDRESSES.lock().withdraw_prefix(pio.prefix);
                                                } else if pio.autonomous {
                                                    let auto = Ipv6Autoconfig::new(pio.prefix, local_mac.0);
                                                    if let Some(slaac_addr) = auto.address() {
                                                        let valid_until = now.saturating_add((pio.valid_lifetime as u64).saturating_mul(1000));
                                                        let pref_until = now.saturating_add((pio.preferred_lifetime as u64).saturating_mul(1000));
                                                        let _ = IPV6_ADDRESSES.lock().assign_tentative(slaac_addr, valid_until, pref_until);

                                                        // RFC 4862 § 5.4: Transmit DAD probe frame for tentative SLAAC address
                                                        let mut dad_buf = [0u8; 1514];
                                                        if let Ok(dad_len) = build_dad_probe_frame(slaac_addr, local_mac, &mut dad_buf) {
                                                            let _ = NETWORK_DEVICE.lock().transmit(&dad_buf[..dad_len]);
                                                        }

                                                        let mut host_ip = HOST_IPV6_LINK_LOCAL.lock();
                                                        if host_ip.is_unspecified() {
                                                            IPV6_ADDRESSES.lock().mark_preferred(slaac_addr);
                                                            *host_ip = slaac_addr;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    128 => { // Echo Request (Ping6)
                                        let host_ip = *HOST_IPV6_LINK_LOCAL.lock();
                                        // Reply source must be unicast host_ip (RFC 4443 § 2.3 / 3.2: never multicast source)
                                        let reply_src = if ip6_pkt.dst.is_multicast() { host_ip } else { ip6_pkt.dst };
                                        if !reply_src.is_unspecified() {
                                            let local_mac = NETWORK_DEVICE.lock().info().mac;
                                            let mut reply_payload = [0u8; 128];
                                            let copy_len = ip6_pkt.payload.len().min(reply_payload.len());
                                            reply_payload[..copy_len].copy_from_slice(&ip6_pkt.payload[..copy_len]);
                                            reply_payload[0] = 129; // Echo Reply
                                            reply_payload[2..4].fill(0);
                                            let csum = calculate_icmpv6_checksum(&reply_src, &ip6_pkt.src, &reply_payload[..copy_len]);
                                            reply_payload[2..4].copy_from_slice(&csum.to_be_bytes());

                                            let ip6_resp = Ipv6Packet {
                                                src: reply_src,
                                                dst: ip6_pkt.src,
                                                next_header: 58,
                                                hop_limit: 64,
                                                payload: &reply_payload[..copy_len],
                                            };
                                            let mut ip6_buf = [0u8; 256];
                                            if let Ok(n_ip6) = ip6_resp.serialize(&mut ip6_buf) {
                                                let eth_resp = EthernetFrame {
                                                    dst_mac: frame.src_mac,
                                                    src_mac: local_mac,
                                                    ethertype: EtherType::Ipv6,
                                                    payload: &ip6_buf[..n_ip6],
                                                };
                                                let mut eth_buf = [0u8; 1514];
                                                if let Ok(n_eth) = eth_resp.serialize(&mut eth_buf) {
                                                    let _ = NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]);
                                                }
                                            }
                                        }
                                    }
                                    137 => { // Redirect
                                        // RFC 4861: Drop ICMPv6 Redirect fail-closed to prevent route poisoning
                                        record_net_drop(NetDropKind::MalformedChecksumOrLength, "icmpv6_redirect_rejected");
                                    }
                                    _ => {
                                        record_net_drop(NetDropKind::UnknownL4Protocol, "unsupported_icmpv6_type");
                                    }
                                }
                            } else {
                                record_net_drop(NetDropKind::MalformedChecksumOrLength, "icmpv6_payload_too_short");
                            }
                        }
                    } else {
                        record_net_drop(NetDropKind::MalformedChecksumOrLength, "ipv6_parse_failed");
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
pub(crate) static NET_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Restores, before a network case runs, only the process globals that the
/// network cases themselves are the only writers and readers of.
///
/// WHY the tables, the ring and the counters: they outlive a single `#[test]`, so
/// whatever one case leaves behind is what the next reads. The loopback ring in
/// particular is `LOOPBACK_RING_SLOTS` frames deep and nothing in the unit-test
/// process ever drains it, so the frames earlier cases transmitted stay queued,
/// and a later case's send fails with `QueueFull` for a reason that has nothing to
/// do with what that case tests.
///
/// WHY the system tick clock and the delegation registry are deliberately NOT
/// touched here: they are read and written by cases outside `net::` that take no
/// part of `NET_TEST_LOCK` -- the capability, revocation, vfs and syscall suites
/// mint leases against the live clock and consult the registry afterwards, and
/// `interrupts::SYSTEM_TICKS` is read by anything that ages a record. Zeroing the
/// clock or clearing the registry between network cases does not make those cases
/// deterministic; it lets a network case delete authority an unrelated case is
/// mid-flight on, which turns an ordering dependence into a genuine race. A
/// partial lock serialises nothing, because an unguarded neighbour is free to
/// interleave with a guarded one.
///
/// So each network case is made immune to whatever is already in the clock and the
/// registry instead: unique lease ids and task ids per case, a lease start anchored
/// to the live clock, and expiry windows far wider than any tick a case can move.
/// The two cases that must observe an expired lease move the clock through
/// `SystemTicksGuard`, which advances it the single tick their own lease needs and
/// puts it back.
///
/// WHY `NET_TEST_LOCK` is still taken by every case that reaches here: two guards
/// running at once would save and restore over each other and leave the clock at
/// the advanced value permanently. Any case outside `net::` that starts driving
/// these tables must take the same lock, or this reset has to go.
#[cfg(test)]
pub(crate) fn reset_net_state() {
    *TCP_TABLE.lock() = TcpConnectionTable::new();
    *SOCKET_TABLE.lock() = SocketTable::new();
    *ARP_TABLE.lock() = ArpTable::new();
    *LOOPBACK_DEVICE.lock() = LoopbackDevice::new();
    *NETWORK_DEVICE.lock() = NetworkDeviceManager::new_loopback();
    *IPV6_NEIGHBOR_CACHE.lock() = NeighborCache::new();
    *IPV6_ROUTING_TABLE.lock() = Ipv6RoutingTable::new();
    *IPV6_ADDRESSES.lock() = crate::net::ipv6::Ipv6AddressTable::new();
    *HOST_IPV6_LINK_LOCAL.lock() = Ipv6Address::UNSPECIFIED;
    *ARP_REQUESTS.lock() = [ArpRequestState::EMPTY; 8];
    // The resolver is a network global like the tables above: its inflight slots
    // decide whether a bind inside the reserved port range is legitimate, so a
    // query one case left outstanding would decide another case's verdict.
    *DNS_RESOLVER.lock() = DnsResolver::new();
    LIVE_GATEWAY_ARP_LOGGED.store(false, Ordering::SeqCst);
    DROP_STATS.net_drop_stats_reset();
}

/// Moves the global tick clock forward for the lifetime of the guard.
///
/// WHY a guard rather than a hand-written store/restore: a lease case advances
/// `SYSTEM_TICKS` to push a short lease past its expiry, and any assertion that
/// fires between the write and the restore line would leave the clock advanced for
/// every later case in the process. `Drop` runs even while unwinding a panic, so
/// the clock returns to exactly what it was whether the case passes or fails.
///
/// WHY the case moves it one tick and not fifty: every other test in this process
/// reads the same clock without taking `NET_TEST_LOCK`, so the window is visible to
/// all of them. A single tick is what a real timer interrupt would do and stays
/// inside every other case's lease window, which is hundreds of ticks wide; a
/// larger leap can expire a neighbour's lease while it is being checked.
#[cfg(test)]
pub(crate) struct SystemTicksGuard {
    prev: u64,
}

#[cfg(test)]
impl SystemTicksGuard {
    pub(crate) fn set(now: u64) -> Self {
        let prev = crate::interrupts::SYSTEM_TICKS.load(core::sync::atomic::Ordering::SeqCst);
        crate::interrupts::SYSTEM_TICKS.store(now, core::sync::atomic::Ordering::SeqCst);
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for SystemTicksGuard {
    fn drop(&mut self) {
        crate::interrupts::SYSTEM_TICKS.store(self.prev, core::sync::atomic::Ordering::SeqCst);
    }
}

/// The task the background-network debit keys to, while a guard holds it.
///
/// `None` means "no override", which is the guest behaviour: read the scheduler.
#[cfg(test)]
pub(crate) static TEST_SEND_CALLER: spin::Mutex<Option<TaskId>> = spin::Mutex::new(None);

/// Gives a case a process the send budget can bill, and points the budget at it.
///
/// WHY the case needs one: `send_udp_datagram_authorized` bills the background
/// network allowance to `SCHEDULER.current_task_id()` -- a process-global the case
/// neither spawns nor owns. As soon as any case has initialized the global scheduler
/// (the syscall 161 dispatch case does), that is the primordial `TaskId(0)`, and
/// `register_leader` refuses `TaskId(0)` by design because it is the "no task"
/// sentinel. The debit then answers "process not found", which the send path reports
/// as `QueueFull` -- so whether a network case could send at all depended on which
/// cases happened to run before it.
///
/// WHY an override rather than a real scheduled context: `current_idx` only moves
/// through `switch_context`, which would switch the host thread's own CPU context,
/// and the alternative to pointing the debit somewhere is for every network case to
/// replay the boot sequence.
///
/// The process itself comes from `task::process::OwnedLeader`, which is what keeps
/// the case's registration from outliving the case.
#[cfg(test)]
pub(crate) struct SendCallerGuard {
    _leader: crate::task::process::OwnedLeader,
    prev: Option<TaskId>,
}

#[cfg(test)]
impl SendCallerGuard {
    /// Bills sends to a freshly allocated process leader until the guard is dropped.
    ///
    /// WHY the id is allocated rather than picked by the case: `OwnedLeader` frees
    /// what it registers, but a hand-picked id can already be bound by an earlier
    /// case's `spawn`, and the guard would then refuse to start.
    pub(crate) fn fresh() -> (TaskId, Self) {
        let (task, leader) = crate::task::process::OwnedLeader::fresh();
        let prev = *TEST_SEND_CALLER.lock();
        *TEST_SEND_CALLER.lock() = Some(task);
        (task, Self { _leader: leader, prev })
    }
}

#[cfg(test)]
impl Drop for SendCallerGuard {
    fn drop(&mut self) {
        *TEST_SEND_CALLER.lock() = self.prev;
    }
}

/// The state the background-network budget debit sees right now, as
/// `(current task, its process, that process's state, bytes debited, allowance)`.
///
/// WHY a case has to be able to print this: `send_udp_datagram_authorized` keys its
/// budget debit to the running task rather than to the capability holder it was
/// handed, and every refusal on that path is reported as `NetError::QueueFull`, which
/// is also what a full transmit ring returns. So an assert that says only "must be
/// authorized: QueueFull" leaves a reader unable to tell a refused budget from a
/// broken send predicate, and the two have nothing to do with each other.
///
/// WHY it reads `caller_task_id()` and not the scheduler directly: that is the
/// function the debit actually uses, and a diagnostic that names a different task
/// than the one that refused is worse than none.
#[cfg(test)]
pub(crate) fn background_debit_state() -> (Option<TaskId>, Option<crate::task::process::ProcessId>, Option<crate::task::process::ProcessState>, u32, u32) {
    let caller = caller_task_id();
    let process = caller.and_then(crate::task::process::process_for_task);
    let record = process.and_then(crate::task::process::record_for_process);
    (
        caller,
        process,
        record.map(|r| r.state),
        record.map(|r| r.background_network_bytes).unwrap_or(0),
        record.map(|r| r.limits.max_background_network_bytes).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Capability, CapabilityId, CapabilityType, Rights, CapabilityTable, DelegatedLease};
    use crate::capability::delegation::{allocate_lease_id, register_lease, MAX_DELEGATION_DURATION_TICKS};
    use crate::task::TaskId;

    #[test]
    fn off_link_destination_is_handed_to_the_router() {
        assert!(is_on_local_link(GATEWAY_IP));
        assert!(is_on_local_link(DEFAULT_HOST_IP));
        assert!(!is_on_local_link(Ipv4Address::new(172, 67, 162, 196)));

        // The user-mode network answers ARP only for its own gateway, so a
        // public address has to be given to the router to leave the kernel at
        // all.
        assert_eq!(next_hop_for(Ipv4Address::new(172, 67, 162, 196)), GATEWAY_IP);
        // A peer on the same prefix is still asked about by name; the router
        // does not answer for hosts that share the wire with us.
        assert_eq!(
            next_hop_for(Ipv4Address::new(10, 0, 2, 3)),
            Ipv4Address::new(10, 0, 2, 3)
        );
    }

    #[test]
    fn net_state_reset_leaves_foreign_process_globals_alone() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // WHY this case exists: `reset_net_state()` runs between network cases, and
        // the tempting version of it also zeroes the tick clock and clears the
        // delegation registry "so nothing stale is left". Those two are process
        // globals that cases outside `net::` mint and consume without ever taking
        // `NET_TEST_LOCK`, so a reset that touches them deletes authority another
        // case is mid-flight on. That is not an ordering quirk to be found by
        // running the suite in a lucky order -- it is a race, and a race only shows
        // up in the run that loses it. So this case does not wait for one: it holds
        // a foreign lease and an advanced clock itself, calls the reset, and checks
        // the reset left both alone. It fails every time on the design that wipes
        // them, and never touches the globals beyond what the guard puts back.
        //
        // WHY the grantee is a task id no case uses: the lease has to stay in the
        // registry to be looked at, and one wide-window lease for a pid nothing else
        // addresses cannot authorize anything for anybody else.
        let grantor = TaskId(0x9100);
        let grantee = TaskId(0x9101);
        let parent = Capability {
            id: CapabilityId(0x9100_0001),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE | Rights::GRANT,
            object_id: 0,
        };
        let first_lease_id = allocate_lease_id();
        let lease = DelegatedLease::new(
            first_lease_id,
            &parent,
            grantor,
            grantee,
            Rights::WRITE,
            4,
            MAX_DELEGATION_DURATION_TICKS,
            crate::interrupts::get_system_ticks(),
        )
        .expect("a wide-window send lease is constructible");
        assert!(register_lease(lease), "the foreign lease must reach the registry");

        let advanced = crate::interrupts::get_system_ticks() + 7;
        let _tick_guard = SystemTicksGuard::set(advanced);

        reset_net_state();

        assert!(
            crate::capability::delegation::is_lease_active(first_lease_id, advanced),
            "reset_net_state() must not clear a lease another subsystem registered: lease {} was live at tick {} and is gone from the registry",
            first_lease_id,
            advanced
        );
        assert_eq!(
            crate::interrupts::get_system_ticks(),
            advanced,
            "reset_net_state() must not rewind the process tick clock: other cases anchor lease expiry to it, so a rewind silently expires or un-expires their authority"
        );

        // WHY the id counter is checked last: handing out an id that is already in
        // use puts two leases under one name, and the quota of one would be spent by
        // the other's calls -- which reads as a lease that "should" still work.
        let second_lease_id = allocate_lease_id();
        assert!(
            second_lease_id > first_lease_id,
            "reset_net_state() must not rewind the lease id counter: it handed out {} after {}, so a fresh lease can collide with one already registered",
            second_lease_id,
            first_lease_id
        );
    }

    #[test]
    fn test_send_udp_datagram_authorized_quota_policy() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let grantor = TaskId::new();
        let (grantee, _caller) = SendCallerGuard::fresh();

        let mut grantor_caps = CapabilityTable::new_for_task(grantor, 4);
        grantor_caps.insert(Capability {
            id: CapabilityId(301),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE | Rights::GRANT,
            object_id: 0,
        }).unwrap();

        // Create a single-use delegated lease for NetworkSend
        // WHY the base is the live clock: expiry is judged against
        // `get_system_ticks()`, so anchoring at `0` would let the process's uptime
        // decide whether this lease is usable, making the case order-dependent.
        let parent_cap = grantor_caps.get(0).unwrap();
        let lease = DelegatedLease::new(
            allocate_lease_id(),
            parent_cap,
            grantor,
            grantee,
            Rights::WRITE,
            1, // 1 invocation quota
            10_000,
            crate::interrupts::get_system_ticks(),
        ).unwrap();
        let lease_cap = lease.to_capability();
        let lease_id = lease.lease_id;
        assert!(register_lease(lease), "single-use send lease must register");

        let mut grantee_caps = CapabilityTable::new_for_task(grantee, 4);
        grantee_caps.insert(lease_cap.clone()).unwrap();

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
        // WHY the refusal is named in the message: this gate has four independent
        // ways to say no (the identity is in the revocation store, the lease is not
        // in the registry, it is revoked there, it is exhausted there) and the bare
        // "must succeed" gave a reader none of them, so a failure here looked like a
        // bug in the retransmission predicate rather than in the shared state.
        //
        // WHY the debit state is in it too: the frame leaves through the same
        // function that spends the ambient process's background allowance, so a
        // `QueueFull` here is not necessarily about the lease at all.
        let (caller, caller_process, caller_state, debited, allowance) = background_debit_state();
        assert!(
            check_res.is_ok(),
            "Non-consuming datagram authorization must succeed: identity {}, in the revocation store {}, live in the registry {}; the budget debit targeted current task {:?} -> process {:?} in state {:?}, {}/{} background bytes used",
            lease_cap.id.0,
            crate::capability::revocation::is_revoked(lease_cap.id),
            crate::capability::delegation::is_lease_active(
                lease_id,
                crate::interrupts::get_system_ticks()
            ),
            caller,
            caller_process,
            caller_state,
            debited,
            allowance,
        );

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
        assert!(send_res1.is_ok(), "First consuming send must succeed: {:?}, debit target {:?}", send_res1.err(), background_debit_state());

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

        // WHY the lease is anchored to the live clock and not to zero:
        // `is_lease_valid` compares expiry against `get_system_ticks()`, not
        // against the case's own notion of now. A lease minted at `0` is born
        // already expired once the process clock has advanced past its duration,
        // so a passing run would depend on how many ticks earlier cases consumed.
        let lease = DelegatedLease::new(
            allocate_lease_id(),
            grantor_caps.get(0).unwrap(),
            grantor,
            grantee,
            Rights::WRITE,
            1,
            10_000,
            crate::interrupts::get_system_ticks(),
        ).unwrap();
        // WHY this is checked: `register_lease` refuses once the registry is at
        // capacity, and a silently-dropped refusal leaves the grantee holding a
        // capability the registry has never heard of — every later authorization
        // then fails for a reason that has nothing to do with what the case tests.
        assert!(register_lease(lease.clone()), "single-use send lease must register");

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
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let (grantee, _caller) = SendCallerGuard::fresh();
        let caps = single_use_send_lease(grantee);

        // Initial transmission spends the single invocation.
        let initial = send_udp_datagram_authorized(
            Ipv4Address::LOCALHOST, 5000, Ipv4Address::LOCALHOST, 4242,
            b"DNS_INITIAL", &caps, true,
        );
        assert!(
            initial.is_ok(),
            "initial transmission must be authorized: {:?}, debit target {:?}",
            initial.err(),
            background_debit_state()
        );

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
    /// explicit caller task, because the budget debit in the public send path is
    /// keyed to the running task rather than to a task the case can name. Driving
    /// the helper is what makes the ordering observable at all: it is the ordering
    /// this test pins, not the frame serialization around it.
    ///
    /// `SendCallerGuard` is here for the process record, not the override: the byte
    /// budget only exists on a process, and the guard reaps the one it registers.
    #[test]
    fn denied_send_does_not_debit_background_network_budget() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let (grantee, _caller) = SendCallerGuard::fresh();
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
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
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

    #[test]
    fn live_rx_dispatch_ndp_hop_limit_and_checksum_enforcement() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let src_mac = MacAddress([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let src_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x00, 0x11, 0x22, 0xff, 0xfe, 0x33, 0x44, 0x55]);
        let target_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

        // Construct NS ICMPv6 payload (type 135, code 0) with SLLA option
        let mut ns_buf = [0u8; 32];
        ns_buf[0] = 135; // NS
        ns_buf[1] = 0;   // code 0
        ns_buf[8..24].copy_from_slice(&target_ip.0);
        ns_buf[24] = 1;  // SLLA option type
        ns_buf[25] = 1;  // length 1 (8 bytes)
        ns_buf[26..32].copy_from_slice(&src_mac.0);

        let valid_csum = calculate_icmpv6_checksum(&src_ip, &target_ip, &ns_buf);
        ns_buf[2..4].copy_from_slice(&valid_csum.to_be_bytes());

        // Case 1: Hop limit 254 (must be rejected with ndp_hop_limit_not_255)
        let ip6_pkt_bad_hl = Ipv6Packet {
            src: src_ip,
            dst: target_ip,
            next_header: 58,
            hop_limit: 254, // Invalid for NDP (RFC 4861 requires 255)
            payload: &ns_buf,
        };
        let mut ip6_raw = [0u8; 128];
        let n_ip6 = ip6_pkt_bad_hl.serialize(&mut ip6_raw).unwrap();
        let eth_frame = EthernetFrame {
            dst_mac: MacAddress::BROADCAST,
            src_mac,
            ethertype: EtherType::Ipv6,
            payload: &ip6_raw[..n_ip6],
        };
        let mut eth_raw = [0u8; 256];
        let n_eth = eth_frame.serialize(&mut eth_raw).unwrap();

        // Enqueue into loopback receive ring
        NETWORK_DEVICE.lock().transmit(&eth_raw[..n_eth]).unwrap();
        let prev_drop = DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed);
        poll_network_interface();

        assert_eq!(
            DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed),
            prev_drop + 1,
            "NDP packet with hop limit < 255 must be dropped"
        );
        assert!(
            IPV6_NEIGHBOR_CACHE.lock().lookup(src_ip, 0).is_none(),
            "Rejected NDP packet must not mutate neighbor cache"
        );

        // Case 2: Hop limit 255 but corrupt checksum (must be rejected with icmpv6_checksum_mismatch)
        let mut corrupted_ns = ns_buf;
        corrupted_ns[2] ^= 0xFF; // corrupt checksum
        let ip6_pkt_bad_csum = Ipv6Packet {
            src: src_ip,
            dst: target_ip,
            next_header: 58,
            hop_limit: 255,
            payload: &corrupted_ns,
        };
        let n_ip6 = ip6_pkt_bad_csum.serialize(&mut ip6_raw).unwrap();
        let eth_frame = EthernetFrame {
            dst_mac: MacAddress::BROADCAST,
            src_mac,
            ethertype: EtherType::Ipv6,
            payload: &ip6_raw[..n_ip6],
        };
        let n_eth = eth_frame.serialize(&mut eth_raw).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_raw[..n_eth]).unwrap();
        let prev_drop = DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed);
        poll_network_interface();

        assert_eq!(
            DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed),
            prev_drop + 1,
            "ICMPv6 packet with invalid checksum must be dropped"
        );
        assert!(
            IPV6_NEIGHBOR_CACHE.lock().lookup(src_ip, 0).is_none(),
            "Corrupt ICMPv6 packet must not mutate neighbor cache"
        );

        // Case 3: Valid hop limit 255 and valid checksum -> must be learned in neighbor cache
        let ip6_pkt_valid = Ipv6Packet {
            src: src_ip,
            dst: target_ip,
            next_header: 58,
            hop_limit: 255,
            payload: &ns_buf,
        };
        let n_ip6 = ip6_pkt_valid.serialize(&mut ip6_raw).unwrap();
        let eth_frame = EthernetFrame {
            dst_mac: MacAddress::BROADCAST,
            src_mac,
            ethertype: EtherType::Ipv6,
            payload: &ip6_raw[..n_ip6],
        };
        let n_eth = eth_frame.serialize(&mut eth_raw).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_raw[..n_eth]).unwrap();
        poll_network_interface();

        assert_eq!(
            IPV6_NEIGHBOR_CACHE.lock().lookup(src_ip, 0),
            Some(src_mac.0),
            "Valid NDP NS must update the neighbor cache"
        );
    }

    /// Injects one TCP segment through the real receive path: Ethernet -> IPv4 ->
    /// the TCP demultiplexer in `poll_network_interface`. Both checksums are
    /// computed, so the segment passes the same validation production traffic does.
    fn inject_tcp_segment(src_ip: Ipv4Address, dst_ip: Ipv4Address, dst_mac: MacAddress, seg: &TcpSegment<'_>) {
        let mut tcp_buf = [0u8; 128];
        let tcp_len = crate::net::tcp::serialize_with_checksum(seg, src_ip.0, dst_ip.0, &mut tcp_buf).unwrap();
        let mut ip_buf = [0u8; 160];
        let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Tcp, 64, &tcp_buf[..tcp_len], &mut ip_buf).unwrap();
        let frame = EthernetFrame {
            dst_mac,
            src_mac: LOOPBACK_MAC,
            ethertype: EtherType::Ipv4,
            payload: &ip_buf[..ip_len],
        };
        let mut eth_buf = [0u8; 1514];
        let n = frame.serialize(&mut eth_buf).unwrap();
        LOOPBACK_DEVICE.lock().transmit(&eth_buf[..n]).unwrap();
    }

    /// Opens a loopback connection and puts it in ESTABLISHED with known sequence
    /// numbers, so a case can state exactly which window a forged segment lands in.
    fn established_loopback_connection(local_port: u16) -> u32 {
        let owner = TaskId::new();
        let id = TCP_TABLE.lock()
            .open(owner.0 as u32, local_port, Ipv4Address::LOCALHOST, 443, 100)
            .expect("connection should open");
        let mut table = TCP_TABLE.lock();
        let conn = table.get_mut(id).expect("connection exists");
        conn.state = TcpState::Established;
        conn.syn_pending = false;
        conn.recv_seq = 5000;
        conn.send_seq = 9000;
        conn.snd_una = 9000;
        conn.rcv_wnd = 4096;
        conn.snd_wnd = 65535;
        id
    }

    /// H5: the attack RFC 5961 exists to stop. One forged RST at a sequence the
    /// attacker never had to guess, carrying a zero advertised window. Before the
    /// fix this closed the connection; even once the reset was refused, the window
    /// update ran first and left the peer unable to send ever again.
    #[test]
    fn a_forged_reset_can_neither_kill_nor_stall_an_established_connection() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let id = established_loopback_connection(41_000);
        let peer = Ipv4Address::LOCALHOST;

        let forged = TcpSegment {
            src_port: 443, dst_port: 41_000, seq: 123_456, ack: 0,
            flags: 0x04, window: 0, payload: &[],
        };
        inject_tcp_segment(peer, peer, LOOPBACK_MAC, &forged);
        let tx_before = LOOPBACK_DEVICE.lock().packets_transmitted;
        let drops_before = DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed);
        poll_network_interface();

        {
            let table = TCP_TABLE.lock();
            let conn = table.get(id).expect("the connection must survive a forged reset");
            assert_eq!(conn.state, TcpState::Established, "an out-of-window reset must not close the connection");
            assert_eq!(conn.recv_seq, 5000, "a refused segment must not move the receive stream");
            assert_eq!(conn.snd_wnd, 65535, "a refused segment must not be allowed to zero the send window");
        }
        assert_eq!(
            LOOPBACK_DEVICE.lock().packets_transmitted, tx_before,
            "an out-of-window reset is dropped silently, so it cannot make us emit traffic"
        );
        assert_eq!(
            DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed), drops_before + 1,
            "the refused reset must be counted, not vanish"
        );

        // Positive control: the very same reset at the exact next expected sequence
        // is believable, so the case proves validation rather than blanket refusal.
        let honest = TcpSegment { seq: 5000, ..forged };
        inject_tcp_segment(peer, peer, LOOPBACK_MAC, &honest);
        poll_network_interface();
        let table = TCP_TABLE.lock();
        assert_eq!(
            table.get(id).expect("the entry survives until the reaper runs").state,
            TcpState::Closed,
            "a reset at recv_seq must still tear the connection down"
        );
    }

    /// H5: an in-window reset is not believed, but it is not ignored either -- the
    /// peer may genuinely have lost state, so it gets our sequence numbers back and
    /// can confirm by resetting at the right one.
    #[test]
    fn an_in_window_reset_draws_a_challenge_ack_and_keeps_the_connection() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let id = established_loopback_connection(41_001);
        let peer = Ipv4Address::LOCALHOST;

        let in_window = TcpSegment {
            src_port: 443, dst_port: 41_001, seq: 5001, ack: 0,
            flags: 0x04, window: 0, payload: &[],
        };
        inject_tcp_segment(peer, peer, LOOPBACK_MAC, &in_window);
        let tx_before = LOOPBACK_DEVICE.lock().packets_transmitted;
        poll_network_interface();

        {
            let table = TCP_TABLE.lock();
            let conn = table.get(id).expect("the connection must survive an in-window reset");
            assert_eq!(conn.state, TcpState::Established);
            assert_eq!(conn.recv_seq, 5000);
        }
        assert_eq!(
            LOOPBACK_DEVICE.lock().packets_transmitted, tx_before + 1,
            "exactly one challenge ACK must go out: no reset, no state change"
        );
    }

    /// H5: the IPv4 layer accepts unicast, broadcast and loopback destinations, so
    /// matching only the port pair let a broadcast-addressed segment drive a
    /// connection bound to a unicast address. The reset here is at exactly recv_seq,
    /// which means it would have been accepted before local_ip was matched.
    #[test]
    fn a_broadcast_addressed_segment_does_not_drive_a_unicast_connection() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let id = established_loopback_connection(41_002);

        let reset = TcpSegment {
            src_port: 443, dst_port: 41_002, seq: 5000, ack: 0,
            flags: 0x04, window: 4096, payload: &[],
        };
        inject_tcp_segment(Ipv4Address::LOCALHOST, Ipv4Address::BROADCAST, MacAddress::BROADCAST, &reset);
        let drops_before = DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed);
        poll_network_interface();

        let table = TCP_TABLE.lock();
        let conn = table.get(id).expect("the connection must not be addressable via broadcast");
        assert_eq!(conn.state, TcpState::Established, "a broadcast segment must not reset a unicast connection");
        assert_eq!(conn.recv_seq, 5000);
        drop(table);
        assert_eq!(
            DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed), drops_before + 1,
            "the segment must fall out of the demux as unmatched"
        );
    }

    /// M4: `acknowledge_fin` tested only a lower bound, so an acknowledgement of
    /// 0xFFFFFFFF retired a FIN that was never acknowledged and closed the
    /// connection. The closing states now window-validate before believing it.
    #[test]
    fn a_forged_ack_cannot_retire_a_pending_fin() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let owner = TaskId::new();
        let id = TCP_TABLE.lock()
            .open(owner.0 as u32, 41_003, Ipv4Address::LOCALHOST, 443, 100)
            .expect("connection should open");
        let fin_seq = {
            let mut table = TCP_TABLE.lock();
            let conn = table.get_mut(id).expect("connection exists");
            conn.state = TcpState::Established;
            conn.syn_pending = false;
            conn.recv_seq = 5000;
            conn.rcv_wnd = 4096;
            // M6: the ISN is kernel-drawn now, so the case states the send side
            // explicitly rather than relying on the value it passed to `open`.
            conn.send_seq = 100;
            conn.snd_una = 100;
            table.arm_fin(id, 0).expect("FIN should arm")
        };
        let peer = Ipv4Address::LOCALHOST;

        let forged = TcpSegment {
            src_port: 443, dst_port: 41_003, seq: 5000, ack: u32::MAX,
            flags: 0x10, window: 4096, payload: &[],
        };
        inject_tcp_segment(peer, peer, LOOPBACK_MAC, &forged);
        poll_network_interface();
        {
            let table = TCP_TABLE.lock();
            let conn = table.get(id).expect("connection exists");
            assert!(conn.fin_pending, "an ack for bytes never sent must not retire the FIN");
            assert_eq!(conn.state, TcpState::FinWait1);
            assert_eq!(conn.recv_seq, 5000);
        }

        // Positive control: the acknowledgement that really covers the FIN works.
        let honest = TcpSegment { ack: fin_seq.wrapping_add(1), ..forged };
        inject_tcp_segment(peer, peer, LOOPBACK_MAC, &honest);
        poll_network_interface();
        let table = TCP_TABLE.lock();
        let conn = table.get(id).expect("connection exists");
        assert!(!conn.fin_pending, "the genuine FIN acknowledgement must still retire it");
        assert_eq!(conn.state, TcpState::TimeWait);
    }

    /// M4, the sequence half: the acknowledgement bound alone does not stop a
    /// closing state from overwriting recv_seq with whatever the wire claimed. The
    /// ack here is in range but does not cover the FIN, so only the window check
    /// stands between the forged sequence and the receive stream.
    #[test]
    fn a_closing_state_does_not_rewind_or_jump_the_receive_stream() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let owner = TaskId::new();
        let id = TCP_TABLE.lock()
            .open(owner.0 as u32, 41_004, Ipv4Address::LOCALHOST, 443, 100)
            .expect("connection should open");
        {
            let mut table = TCP_TABLE.lock();
            let conn = table.get_mut(id).expect("connection exists");
            conn.state = TcpState::Established;
            conn.syn_pending = false;
            conn.recv_seq = 5000;
            conn.rcv_wnd = 4096;
            // M6: as above, the send side is stated rather than inherited from
            // the caller's now-ignored initial sequence argument.
            conn.send_seq = 100;
            conn.snd_una = 100;
            table.arm_fin(id, 0).expect("FIN should arm");
        }
        let peer = Ipv4Address::LOCALHOST;

        for seq in [0u32, 4999, 99_999, u32::MAX] {
            let forged = TcpSegment {
                src_port: 443, dst_port: 41_004, seq, ack: 100,
                flags: 0x10, window: 4096, payload: &[],
            };
            inject_tcp_segment(peer, peer, LOOPBACK_MAC, &forged);
            poll_network_interface();
            let table = TCP_TABLE.lock();
            let conn = table.get(id).expect("connection exists");
            assert_eq!(conn.recv_seq, 5000, "a segment at {seq} is outside the window and must not move recv_seq");
            assert!(conn.fin_pending, "an ack that does not cover the FIN must not retire it");
            assert_eq!(conn.state, TcpState::FinWait1);
        }
    }

    #[test]
    fn live_rx_dispatch_arp_cross_layer_mac_consistency() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let eth_src_mac = MacAddress([0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]);
        let arp_claimed_mac = MacAddress([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let test_ip = Ipv4Address([10, 0, 2, 42]);

        // Frame where Ethernet header source MAC != ARP sender MAC
        let arp_pkt = ArpPacket {
            opcode: ArpOpcode::Reply,
            sender_mac: arp_claimed_mac,
            sender_ip: test_ip,
            target_mac: MacAddress([0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
            target_ip: DEFAULT_HOST_IP,
        };
        let mut arp_buf = [0u8; 28];
        let n_arp = arp_pkt.serialize(&mut arp_buf).unwrap();

        let eth_frame = EthernetFrame {
            dst_mac: MacAddress([0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
            src_mac: eth_src_mac, // Inconsistent with arp_pkt.sender_mac!
            ethertype: EtherType::Arp,
            payload: &arp_buf[..n_arp],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = eth_frame.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        let prev_drop = DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed);
        poll_network_interface();

        assert_eq!(
            DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed),
            prev_drop + 1,
            "Inconsistent ARP sender MAC vs Ethernet source MAC must be dropped"
        );
        assert!(
            ARP_TABLE.lock().lookup(test_ip).is_none(),
            "Inconsistent ARP packet must not be entered into the ARP table"
        );
    }

    /// M2: the in-flight correlation gate is unchanged -- an unsolicited reply is
    /// still dropped, and a reply correlated with a request for a *different*
    /// address still cannot rewrite the gateway. What changed is that the entry
    /// the solicited reply creates is bounded and replaceable, not permanent.
    #[test]
    fn test_unsolicited_arp_reply_rejected_and_solicited_learns_a_bounded_gateway_mapping() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let attacker_mac = MacAddress([0x00, 0x50, 0x56, 0x11, 0x22, 0x33]);
        let gw_mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        let host_mac = NETWORK_DEVICE.lock().info().mac;

        // 1. Unsolicited ARP reply claiming to be GATEWAY_IP without prior request
        let arp_unsolicited = ArpPacket {
            opcode: ArpOpcode::Reply,
            sender_mac: attacker_mac,
            sender_ip: GATEWAY_IP,
            target_mac: host_mac,
            target_ip: DEFAULT_HOST_IP,
        };
        let mut arp_buf = [0u8; 28];
        let n_arp = arp_unsolicited.serialize(&mut arp_buf).unwrap();
        let eth_frame = EthernetFrame {
            dst_mac: host_mac,
            src_mac: attacker_mac,
            ethertype: EtherType::Arp,
            payload: &arp_buf[..n_arp],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = eth_frame.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        poll_network_interface();

        assert!(
            ARP_TABLE.lock().lookup(GATEWAY_IP).is_none(),
            "Unsolicited ARP reply for gateway must be rejected"
        );

        // 2. Transmit legitimate request, then provide solicited reply
        let _ = transmit_arp_request(GATEWAY_IP);
        let arp_solicited = ArpPacket {
            opcode: ArpOpcode::Reply,
            sender_mac: gw_mac,
            sender_ip: GATEWAY_IP,
            target_mac: host_mac,
            target_ip: DEFAULT_HOST_IP,
        };
        let n_arp = arp_solicited.serialize(&mut arp_buf).unwrap();
        let eth_frame = EthernetFrame {
            dst_mac: host_mac,
            src_mac: gw_mac,
            ethertype: EtherType::Arp,
            payload: &arp_buf[..n_arp],
        };
        let n_eth = eth_frame.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        poll_network_interface();

        assert_eq!(
            ARP_TABLE.lock().lookup(GATEWAY_IP),
            Some(gw_mac),
            "Solicited ARP reply for gateway must be learned with a bounded lifetime"
        );

        // 3. A reply correlated with a request for another address still cannot
        //    rewrite the gateway entry: the in-flight gate is what stops it.
        let _ = transmit_arp_request(Ipv4Address([10, 0, 2, 99])); // request for unrelated IP
        let arp_poison = ArpPacket {
            opcode: ArpOpcode::Reply,
            sender_mac: attacker_mac,
            sender_ip: GATEWAY_IP,
            target_mac: host_mac,
            target_ip: DEFAULT_HOST_IP,
        };
        let n_arp = arp_poison.serialize(&mut arp_buf).unwrap();
        let eth_frame = EthernetFrame {
            dst_mac: host_mac,
            src_mac: attacker_mac,
            ethertype: EtherType::Arp,
            payload: &arp_buf[..n_arp],
        };
        let n_eth = eth_frame.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        poll_network_interface();

        assert_eq!(
            ARP_TABLE.lock().lookup(GATEWAY_IP),
            Some(gw_mac),
            "An uncorrelated reply must not overwrite the gateway entry"
        );
    }

    /// Delivers one ARP reply through the real receive path, unicast to our MAC.
    fn deliver_arp_reply(sender_ip: Ipv4Address, sender_mac: MacAddress) {
        let host_mac = NETWORK_DEVICE.lock().info().mac;
        let reply = ArpPacket {
            opcode: ArpOpcode::Reply,
            sender_mac,
            sender_ip,
            target_mac: host_mac,
            target_ip: DEFAULT_HOST_IP,
        };
        let mut arp_buf = [0u8; 28];
        let n_arp = reply.serialize(&mut arp_buf).unwrap();
        let frame = EthernetFrame {
            dst_mac: host_mac,
            src_mac: sender_mac,
            ethertype: EtherType::Arp,
            payload: &arp_buf[..n_arp],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = frame.serialize(&mut eth_buf).unwrap();
        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        poll_network_interface();
    }

    /// M2: the defect was permanence, not the absence of a cache. The first
    /// responder to a correlated request used to be written with
    /// `insert_permanent`, which `arp.rs` then refuses to overwrite, so one
    /// spoofed reply that won the race owned the next hop for the whole uptime.
    /// A later correlated reply must replace it, and the entry must age out.
    #[test]
    fn a_gateway_mapping_is_replaceable_and_expires() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let first_mac = MacAddress([0x00, 0x50, 0x56, 0xaa, 0xbb, 0xcc]);
        let real_mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        let _ = transmit_arp_request(GATEWAY_IP);
        deliver_arp_reply(GATEWAY_IP, first_mac);
        assert_eq!(ARP_TABLE.lock().lookup(GATEWAY_IP), Some(first_mac));
        assert!(
            !ARP_TABLE.lock().is_permanent(GATEWAY_IP),
            "the gateway must not be pinned permanently: that is what made a single spoofed reply a persistent MITM"
        );

        // The real gateway answers a later request. Under the permanent pin this
        // reply was discarded and the attacker's MAC survived for the whole uptime.
        let _ = transmit_arp_request(GATEWAY_IP);
        deliver_arp_reply(GATEWAY_IP, real_mac);
        assert_eq!(
            ARP_TABLE.lock().lookup(GATEWAY_IP),
            Some(real_mac),
            "a stale gateway mapping must be replaceable by a later correlated reply"
        );

        // And it ages on the same bounded lifetime as every other neighbour.
        let later = crate::interrupts::get_system_ticks()
            .saturating_add(crate::net::arp::ARP_CACHE_TICKS.saturating_mul(2));
        assert_eq!(
            ARP_TABLE.lock().lookup_at(GATEWAY_IP, later),
            None,
            "the gateway mapping must expire, so the host re-ARPs instead of trusting it forever"
        );

        // The correlation gate is intact: a reply correlated with a request for
        // another address cannot rewrite the gateway even now that it is dynamic.
        let _ = transmit_arp_request(Ipv4Address([10, 0, 2, 99]));
        deliver_arp_reply(GATEWAY_IP, first_mac);
        assert_eq!(
            ARP_TABLE.lock().lookup(GATEWAY_IP),
            Some(real_mac),
            "an uncorrelated reply must still be refused"
        );
    }

    /// M6: the ISN is drawn by the kernel inside `open`, but the connect syscall
    /// still forwards its caller's value to the transmit path. The wire and the
    /// table must agree, or `valid_syn_ack` compares against a sequence the peer
    /// never saw and the connection can never establish.
    #[test]
    fn the_syn_on_the_wire_carries_the_kernel_isn_and_the_handshake_still_completes() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let owner = TaskId::new();
        let peer = Ipv4Address::LOCALHOST;
        let id = TCP_TABLE.lock()
            .open(owner.0 as u32, 41_010, peer, 443, 100)
            .expect("connection should open");
        let isn = TCP_TABLE.lock().get(id).expect("connection exists").syn_seq;
        assert_ne!(isn, 100, "the kernel must not put the caller's value on the wire");

        // The caller's value is what the syscall forwards; the table is the authority.
        transmit_tcp_syn_with_window(peer, 41_010, peer, 443, 100, 65535)
            .expect("the SYN transmits");
        let mut buf = [0u8; 1514];
        let n = LOOPBACK_DEVICE.lock().receive(&mut buf).expect("the SYN is queued");
        let frame = EthernetFrame::parse(&buf[..n]).expect("ethernet frame");
        let ip = Ipv4Packet::parse(frame.payload).expect("ipv4 packet");
        let syn = TcpSegment::parse(ip.payload).expect("tcp segment");
        assert_eq!(syn.seq, isn, "the SYN on the wire must carry the connection's own ISN");
        assert_eq!(syn.flags, 0x02);

        // Positive control: the peer acknowledges that ISN and the connection
        // establishes exactly as it did before the kernel took the draw over.
        let syn_ack = TcpSegment {
            src_port: 443, dst_port: 41_010, seq: 7000, ack: isn.wrapping_add(1),
            flags: 0x12, window: 4096, payload: &[],
        };
        inject_tcp_segment(peer, peer, LOOPBACK_MAC, &syn_ack);
        poll_network_interface();
        let table = TCP_TABLE.lock();
        let conn = table.get(id).expect("connection exists");
        assert_eq!(conn.state, TcpState::Established, "a kernel-drawn ISN must still complete the handshake");
        assert!(!conn.syn_pending);
        assert_eq!(conn.send_seq, isn.wrapping_add(1));
        assert_eq!(conn.recv_seq, 7001);
    }

    /// M6: without entropy the connection does not open. This drives the
    /// production refusal branch -- `net_entropy_u64` declines a contended TRNG
    /// rather than waiting -- and checks no slot or quota was consumed.
    #[test]
    fn a_connection_is_refused_when_entropy_cannot_be_drawn() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();
        let owner = TaskId::new();
        let _held = crate::crypto::TRNG.lock();
        assert_eq!(
            TCP_TABLE.lock().open(owner.0 as u32, 41_011, Ipv4Address::LOCALHOST, 443, 100),
            Err(NetError::NotReady),
            "a predictable ISN is worse than no connection"
        );
        assert!(TCP_TABLE.lock().entries().iter().flatten().next().is_none());
    }

    #[test]
    fn test_ethernet_unicast_dst_mac_filtering() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let wrong_dst_mac = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x99]);
        let src_mac = MacAddress([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let target_ip = Ipv4Address([10, 0, 2, 50]);

        let arp_pkt = ArpPacket {
            opcode: ArpOpcode::Request,
            sender_mac: src_mac,
            sender_ip: target_ip,
            target_mac: MacAddress::ZERO,
            target_ip: DEFAULT_HOST_IP,
        };
        let mut arp_buf = [0u8; 28];
        let n_arp = arp_pkt.serialize(&mut arp_buf).unwrap();

        // Frame destination MAC is unicast but not ours!
        let eth_frame = EthernetFrame {
            dst_mac: wrong_dst_mac,
            src_mac,
            ethertype: EtherType::Arp,
            payload: &arp_buf[..n_arp],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = eth_frame.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        let prev_drop = DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed);
        poll_network_interface();

        assert_eq!(
            DROP_STATS.port_conn_mismatch.load(Ordering::Relaxed),
            prev_drop + 1,
            "Unicast frame for different MAC must be dropped"
        );
        assert!(
            ARP_TABLE.lock().lookup(target_ip).is_none(),
            "Dropped frame must not update ARP table"
        );
    }

    #[test]
    fn test_icmpv6_solicited_na_multicast_discarded() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let src_mac = MacAddress([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let src_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x00, 0x11, 0x22, 0xff, 0xfe, 0x33, 0x44, 0x55]);
        let target_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);

        // NA with Solicited=1 (0x40) sent to multicast destination ff02::1
        let mut na_buf = [0u8; 32];
        na_buf[0] = 136; // NA
        na_buf[1] = 0;
        na_buf[4] = 0x40; // Solicited=1
        na_buf[8..24].copy_from_slice(&target_ip.0);
        na_buf[24] = 2; // TLLA
        na_buf[25] = 1;
        na_buf[26..32].copy_from_slice(&src_mac.0);

        let csum = calculate_icmpv6_checksum(&src_ip, &Ipv6Address::ALL_NODES, &na_buf);
        na_buf[2..4].copy_from_slice(&csum.to_be_bytes());

        let ip6 = Ipv6Packet {
            src: src_ip,
            dst: Ipv6Address::ALL_NODES, // Multicast destination!
            next_header: 58,
            hop_limit: 255,
            payload: &na_buf,
        };
        let mut ip6_buf = [0u8; 128];
        let n_ip6 = ip6.serialize(&mut ip6_buf).unwrap();

        let eth = EthernetFrame {
            dst_mac: MacAddress([0x33, 0x33, 0x00, 0x00, 0x00, 0x01]),
            src_mac,
            ethertype: EtherType::Ipv6,
            payload: &ip6_buf[..n_ip6],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = eth.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        let prev_drop = DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed);
        poll_network_interface();

        assert_eq!(
            DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed),
            prev_drop + 1,
            "Solicited NA sent to multicast destination must be silently dropped"
        );
        assert!(
            IPV6_NEIGHBOR_CACHE.lock().lookup(target_ip, 0).is_none(),
            "Dropped NA must not update neighbor cache"
        );
    }

    #[test]
    fn test_icmpv6_redirect_rejected_fail_closed() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let router_mac = MacAddress([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]);
        let router_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x00, 0x11, 0x22, 0xff, 0xfe, 0x33, 0x44, 0x55]);
        let host_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        *HOST_IPV6_LINK_LOCAL.lock() = host_ip;

        // Redirect message (Type 137, Code 0)
        let mut redir_buf = [0u8; 40];
        redir_buf[0] = 137;
        redir_buf[1] = 0;
        let csum = calculate_icmpv6_checksum(&router_ip, &host_ip, &redir_buf);
        redir_buf[2..4].copy_from_slice(&csum.to_be_bytes());

        let ip6 = Ipv6Packet {
            src: router_ip,
            dst: host_ip,
            next_header: 58,
            hop_limit: 255,
            payload: &redir_buf,
        };
        let mut ip6_buf = [0u8; 128];
        let n_ip6 = ip6.serialize(&mut ip6_buf).unwrap();

        let eth = EthernetFrame {
            dst_mac: NETWORK_DEVICE.lock().info().mac,
            src_mac: router_mac,
            ethertype: EtherType::Ipv6,
            payload: &ip6_buf[..n_ip6],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = eth.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        let prev_drop = DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed);
        poll_network_interface();

        assert_eq!(
            DROP_STATS.malformed_checksum_length.load(Ordering::Relaxed),
            prev_drop + 1,
            "ICMPv6 Redirect must be rejected fail-closed"
        );
    }

    #[test]
    fn test_icmpv6_dad_probe_invalidates_tentative_address() {
        let _net_guard = NET_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_net_state();

        let tentative_ip = Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 77]);
        IPV6_ADDRESSES.lock().assign_tentative(tentative_ip, 100_000, 100_000).unwrap();
        assert!(IPV6_ADDRESSES.lock().contains_tentative(tentative_ip));

        // Inbound DAD probe: NS from :: for tentative_ip
        let mut ns_buf = [0u8; 24];
        ns_buf[0] = 135;
        ns_buf[1] = 0;
        ns_buf[8..24].copy_from_slice(&tentative_ip.0);
        let csum = calculate_icmpv6_checksum(&Ipv6Address::UNSPECIFIED, &tentative_ip.solicited_node_multicast(), &ns_buf);
        ns_buf[2..4].copy_from_slice(&csum.to_be_bytes());

        let ip6 = Ipv6Packet {
            src: Ipv6Address::UNSPECIFIED,
            dst: tentative_ip.solicited_node_multicast(),
            next_header: 58,
            hop_limit: 255,
            payload: &ns_buf,
        };
        let mut ip6_buf = [0u8; 128];
        let n_ip6 = ip6.serialize(&mut ip6_buf).unwrap();

        let eth = EthernetFrame {
            dst_mac: tentative_ip.solicited_node_multicast().multicast_mac(),
            src_mac: MacAddress([0x02, 0x11, 0x22, 0x33, 0x44, 0x55]),
            ethertype: EtherType::Ipv6,
            payload: &ip6_buf[..n_ip6],
        };
        let mut eth_buf = [0u8; 1514];
        let n_eth = eth.serialize(&mut eth_buf).unwrap();

        NETWORK_DEVICE.lock().transmit(&eth_buf[..n_eth]).unwrap();
        poll_network_interface();

        assert!(
            !IPV6_ADDRESSES.lock().contains_tentative(tentative_ip),
            "DAD collision must mark tentative address invalid"
        );
    }
}

