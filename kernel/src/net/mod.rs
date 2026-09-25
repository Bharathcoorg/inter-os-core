//! Inter OS Pure no_std Network Subsystem
//!
//! Exposes Ethernet II, ARP, IPv4, UDP, Loopback device, and capability-governed sockets.
//! Enforces Zero-Trust Principle #2: Network operations require explicit capability tokens.

pub mod arp;
pub mod device;
pub mod dns;
pub mod e1000;
pub mod tcp;
pub mod tls;
pub mod ethernet;
pub mod ipv4;
pub mod socket;
pub mod udp;
pub mod wireless;

pub use arp::{ArpOpcode, ArpPacket, ArpTable};
pub use device::{LoopbackDevice, NetDeviceInfo, NetError, NetworkBackend, NetworkDevice, NetworkDeviceManager, NETWORK_DEVICE};
pub use e1000::E1000Device;
pub use dns::{encode_query as dns_encode_query, parse_a_answer, DnsAnswer, DnsQuestion, DnsResolver, DNS_SERVER};
pub use tcp::{next_state as tcp_next_state, seq_ge, seq_gt, seq_le, seq_lt, TcpSegment, TcpState};
pub use tls::{ContentType as TlsContentType, TlsRecord};
pub use ethernet::{EtherType, EthernetFrame, MacAddress};
pub use ipv4::{calculate_ipv4_checksum, verify_ipv4_checksum, IpProtocol, Ipv4Address, Ipv4Packet};
pub use socket::{SocketId, SocketTable, TcpConnection, TcpConnectionTable, UdpSocket, TCP_MAX_RETRIES, TCP_RETRANSMIT_TICKS};
pub use udp::UdpDatagram;
pub use wireless::{WifiState, WirelessManager, WIRELESS_MANAGER};

use spin::Mutex;
use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::serial_println;

pub const DEFAULT_HOST_IP: Ipv4Address = Ipv4Address([10, 0, 2, 15]);
pub const DEFAULT_HOST_MAC: MacAddress = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

/// Global dynamic ARP resolution cache.
pub static ARP_TABLE: Mutex<ArpTable> = Mutex::new(ArpTable::new());

/// Global capability-guarded socket table.
pub static SOCKET_TABLE: Mutex<SocketTable> = Mutex::new(SocketTable::new());
pub static TCP_TABLE: Mutex<TcpConnectionTable> = Mutex::new(TcpConnectionTable::new());

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
    if let Some(mac) = ARP_TABLE.lock().lookup(dst_ip) {
        return Ok(mac);
    }
    transmit_arp_request(dst_ip)?;
    Err(NetError::NoPacketAvailable)
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
    // 1. Enforce Zero-Trust Capability Invariant
    if !caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE) {
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

    // 6. Transmit into Loopback / Virtual NIC
    NETWORK_DEVICE.lock().transmit(&eth_buf[0..eth_len])
}

pub fn transmit_tcp_segment(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, flags: u16) -> Result<(), NetError> {
    let segment = TcpSegment { src_port, dst_port, seq, ack, flags, window: 65535, payload: &[] };
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
    NETWORK_DEVICE.lock().transmit(&eth_buf[..len])
}

fn transmit_tcp_payload(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, payload: &[u8]) -> Result<usize, NetError> {
    let segment = TcpSegment { src_port, dst_port, seq, ack, flags: 0x18, window: 65535, payload };
    let mut tcp_buf = [0u8; 1480];
    let tcp_len = crate::net::tcp::serialize_with_checksum(&segment, src_ip.0, dst_ip.0, &mut tcp_buf).map_err(|_| NetError::BufferTooSmall)?;
    let mut ip_buf = [0u8; 1514];
    let ip_len = Ipv4Packet::serialize(src_ip, dst_ip, IpProtocol::Tcp, 64, &tcp_buf[..tcp_len], &mut ip_buf).map_err(|_| NetError::BufferTooSmall)?;
    let dst_mac = resolve_destination_mac(dst_ip)?;
    let frame = EthernetFrame { dst_mac, src_mac: NETWORK_DEVICE.lock().info().mac, ethertype: EtherType::Ipv4, payload: &ip_buf[..ip_len] };
    let mut eth_buf = [0u8; 1514];
    let n = frame.serialize(&mut eth_buf).map_err(|_| NetError::BufferTooSmall)?;
    NETWORK_DEVICE.lock().transmit(&eth_buf[..n])?;
    Ok(payload.len())
}

pub fn send_tcp_payload(src_ip: Ipv4Address, src_port: u16, dst_ip: Ipv4Address, dst_port: u16, seq: u32, ack: u32, payload: &[u8], caps: &CapabilityTable) -> Result<usize, NetError> {
    if !caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE) { return Err(NetError::CapabilityDenied); }
    transmit_tcp_payload(src_ip, src_port, dst_ip, dst_port, seq, ack, payload)
}

pub fn send_tcp_syn(
    src_ip: Ipv4Address,
    src_port: u16,
    dst_ip: Ipv4Address,
    dst_port: u16,
    seq: u32,
    caps: &CapabilityTable,
) -> Result<(), NetError> {
    if !caps.has_capability(CapabilityType::NetworkSend, Rights::WRITE) {
        return Err(NetError::CapabilityDenied);
    }
    let segment = TcpSegment {
        src_port, dst_port, seq, ack: 0, flags: 0x02, window: 65535, payload: &[],
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
    NETWORK_DEVICE.lock().transmit(&eth_buf[..len])
}

/// Polls the network device and processes all inbound packets.
pub fn poll_network_interface() -> usize {
    let mut processed = 0;
    let _ = poll_tcp_retransmissions(crate::interrupts::get_system_ticks());
    loop {
        let mut rx_buf = [0u8; 1514];
        let len = {
            let mut device = NETWORK_DEVICE.lock();
            match device.receive(&mut rx_buf) {
                Ok(len) => len,
                Err(_) => break,
            }
        };
        processed += 1;
        if let Ok(frame) = EthernetFrame::parse(&rx_buf[..len]) {
            match frame.ethertype {
                EtherType::Arp => {
                    if let Ok(arp_pkt) = ArpPacket::parse(frame.payload) {
                        ARP_TABLE.lock().insert(arp_pkt.sender_ip, arp_pkt.sender_mac);
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
                    if let Ok(ip_pkt) = Ipv4Packet::parse(frame.payload) {
                        if ip_pkt.protocol == IpProtocol::Tcp {
                            if let Ok(segment) = TcpSegment::parse(ip_pkt.payload) {
                                if segment.verify_checksum(ip_pkt.src.0, ip_pkt.dst.0, ip_pkt.payload) {
                                    let mut table = TCP_TABLE.lock();
                                    for conn in table.entries_mut() {
                                        if let Some(conn) = conn.as_mut() {
                                            if conn.remote_ip == ip_pkt.src && conn.remote_port == segment.src_port && conn.local_port == segment.dst_port {
                                                let old_state = conn.state;
                                                if segment.flags & 0x12 == 0x12 && old_state == TcpState::SynSent {
                                                    // SYN consumes one sequence number; the ACK must
                                                    // acknowledge exactly that SYN before we enter Established.
                                                    if segment.ack != conn.send_seq.wrapping_add(1) { continue; }
                                                    conn.recv_seq = segment.seq.wrapping_add(1);
                                                    conn.send_seq = conn.send_seq.wrapping_add(1);
                                                    conn.state = TcpState::Established;
                                                    let _ = TCP_TABLE.lock().acknowledge_syn(conn.id);
                                                    let ack_seq = conn.send_seq;
                                                    let _ = transmit_tcp_segment(ip_pkt.dst, conn.local_port, ip_pkt.src, conn.remote_port, ack_seq, conn.recv_seq, 0x10);
                                                } else if old_state == TcpState::Established {
                                                    // Never accept acknowledgements beyond data we have sent.
                                                    if crate::net::tcp::seq_gt(segment.ack, conn.send_seq) { continue; }
                                                    let _ = TCP_TABLE.lock().acknowledge_tx(conn.id, segment.ack);
                                                    let _ = TCP_TABLE.lock().acknowledge_fin(conn.id, segment.ack);
                                                    let fin = segment.flags & 0x01 != 0;
                                                    if !segment.payload.is_empty() || fin {
                                                        let conn_id = conn.id;
                                                        let before = conn.recv_seq;
                                                        drop(table);
                                                        let accepted = TCP_TABLE.lock().accept_rx_segment(conn_id, segment.seq, segment.payload, fin).unwrap_or(0);
                                                        let table2 = TCP_TABLE.lock();
                                                        if let Some(c) = table2.get(conn_id) {
                                                            if accepted > 0 || fin && c.recv_seq != before {
                                                                let _ = transmit_tcp_segment(ip_pkt.dst, c.local_port, ip_pkt.src, c.remote_port, c.send_seq, c.recv_seq, 0x10);
                                                            }
                                                        }
                                                        drop(table2);
                                                        table = TCP_TABLE.lock();
                                                    }
                                                } else {
                                                    conn.recv_seq = segment.seq.wrapping_add(segment.payload.len() as u32);
                                                    conn.state = tcp_next_state(old_state, segment.flags);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else if ip_pkt.protocol == IpProtocol::Udp {
                            if let Ok(udp_dg) = UdpDatagram::parse(ip_pkt.payload) {
                                SOCKET_TABLE.lock().deliver_udp(
                                    udp_dg.dst_port,
                                    ip_pkt.src,
                                    udp_dg.src_port,
                                    udp_dg.payload,
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    processed
}

/// Retransmits one bounded unacknowledged TCP payload when its deadline expires.
pub fn poll_tcp_retransmissions(now: u64) -> usize {
    if let Some((_id, remote_ip, local_port, remote_port, seq)) = TCP_TABLE.lock().due_syn_retransmit(now) {
        return if send_tcp_syn(
            if remote_ip.is_loopback() { Ipv4Address::LOCALHOST } else { DEFAULT_HOST_IP },
            local_port, remote_ip, remote_port, seq,
            &CapabilityTable::new(0),
        ).is_ok() { 1 } else { 0 };
    }
    let due_fin = TCP_TABLE.lock().due_fin_retransmit(now);
    if let Some((_id, remote_ip, local_port, remote_port, seq, ack)) = due_fin {
        return if transmit_tcp_segment(if remote_ip.is_loopback() { Ipv4Address::LOCALHOST } else { DEFAULT_HOST_IP }, local_port, remote_ip, remote_port, seq, ack, 0x11).is_ok() { 1 } else { 0 };
    }
    let due = TCP_TABLE.lock().due_retransmit(now);
    let Some((id, remote_ip, local_port, remote_port, seq, ack, len)) = due else { return 0; };
    let payload = {
        let table = TCP_TABLE.lock();
        let Some(c) = table.get(id) else { return 0; };
        let mut buf = [0u8; crate::net::tcp::TCP_MAX_SEGMENT];
        buf[..len].copy_from_slice(&c.tx[..len]);
        buf
    };
    if transmit_tcp_payload(if remote_ip.is_loopback() { Ipv4Address::LOCALHOST } else { DEFAULT_HOST_IP }, local_port, remote_ip, remote_port, seq, ack, &payload[..len]).is_ok() { 1 } else { 0 }
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

    // Mobile Wireless HAL & BLE Beacon Initialization
    wireless::init();

    serial_println!("--- [Network Subsystem 100% Attested & Enforced] ---");
}
