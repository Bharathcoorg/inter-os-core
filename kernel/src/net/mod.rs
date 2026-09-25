//! Inter OS Pure no_std Network Subsystem
//!
//! Exposes Ethernet II, ARP, IPv4, UDP, Loopback device, and capability-governed sockets.
//! Enforces Zero-Trust Principle #2: Network operations require explicit capability tokens.

pub mod arp;
pub mod device;
pub mod ethernet;
pub mod ipv4;
pub mod socket;
pub mod udp;
pub mod wireless;

pub use arp::{ArpOpcode, ArpPacket, ArpTable};
pub use device::{LoopbackDevice, NetDeviceInfo, NetError, NetworkBackend, NetworkDevice, NetworkDeviceManager, NETWORK_DEVICE};
pub use ethernet::{EtherType, EthernetFrame, MacAddress};
pub use ipv4::{calculate_ipv4_checksum, verify_ipv4_checksum, IpProtocol, Ipv4Address, Ipv4Packet};
pub use socket::{SocketId, SocketTable, UdpSocket};
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
    let dst_mac = if dst_ip.is_loopback() {
        MacAddress([0x00, 0x00, 0x00, 0x00, 0x00, 0x01])
    } else {
        let arp = ARP_TABLE.lock();
        arp.lookup(dst_ip).unwrap_or(MacAddress::BROADCAST)
    };

    // 5. Serialize Ethernet Frame
    let mut eth_buf = [0u8; 1514];
    let frame = EthernetFrame {
        dst_mac,
        src_mac: DEFAULT_HOST_MAC,
        ethertype: EtherType::Ipv4,
        payload: &ip_buf[0..ip_len],
    };
    let eth_len = frame.serialize(&mut eth_buf).map_err(|_| NetError::BufferTooSmall)?;

    // 6. Transmit into Loopback / Virtual NIC
    NETWORK_DEVICE.lock().transmit(&eth_buf[0..eth_len])
}

/// Polls the network device and processes all inbound packets.
pub fn poll_network_interface() -> usize {
    let mut device = NETWORK_DEVICE.lock();
    let mut rx_buf = [0u8; 1514];
    let mut processed = 0;

    while let Ok(len) = device.receive(&mut rx_buf) {
        processed += 1;
        let frame_res = EthernetFrame::parse(&rx_buf[0..len]);
        if let Ok(frame) = frame_res {
            match frame.ethertype {
                EtherType::Arp => {
                    if let Ok(arp_pkt) = ArpPacket::parse(frame.payload) {
                        if arp_pkt.opcode == ArpOpcode::Reply || arp_pkt.opcode == ArpOpcode::Request {
                            ARP_TABLE.lock().insert(arp_pkt.sender_ip, arp_pkt.sender_mac);
                        }
                    }
                }
                EtherType::Ipv4 => {
                    if let Ok(ip_pkt) = Ipv4Packet::parse(frame.payload) {
                        if ip_pkt.protocol == IpProtocol::Udp {
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
