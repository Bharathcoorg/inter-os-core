//! IPv4 (Internet Protocol Version 4) Packet Parsing & Checksum Engine
//!
//! Complies with RFC 791. Implements Internet Checksum calculation and verification.

use core::fmt;

/// 32-bit IPv4 Network Address
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ipv4Address(pub [u8; 4]);

impl Ipv4Address {
    pub const LOCALHOST: Ipv4Address = Ipv4Address([127, 0, 0, 1]);
    pub const UNSPECIFIED: Ipv4Address = Ipv4Address([0, 0, 0, 0]);
    pub const BROADCAST: Ipv4Address = Ipv4Address([255, 255, 255, 255]);

    pub const fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Ipv4Address([a, b, c, d])
    }

    pub fn is_loopback(&self) -> bool {
        self.0[0] == 127
    }
}

impl fmt::Debug for Ipv4Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

impl fmt::Display for Ipv4Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// IPv4 Transport Protocol Numbers
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IpProtocol {
    Icmp = 1,
    Tcp = 6,
    Udp = 17,
    Unknown(u8),
}

impl From<u8> for IpProtocol {
    fn from(val: u8) -> Self {
        match val {
            1 => IpProtocol::Icmp,
            6 => IpProtocol::Tcp,
            17 => IpProtocol::Udp,
            other => IpProtocol::Unknown(other),
        }
    }
}

impl From<IpProtocol> for u8 {
    fn from(proto: IpProtocol) -> Self {
        match proto {
            IpProtocol::Icmp => 1,
            IpProtocol::Tcp => 6,
            IpProtocol::Udp => 17,
            IpProtocol::Unknown(v) => v,
        }
    }
}

pub const IPV4_MIN_HEADER_LEN: usize = 20;

/// RFC 791 Standard Internet Checksum Calculation (16-bit One's Complement Sum)
pub fn calculate_ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i + 1 < header.len() {
        let word = u16::from_be_bytes([header[i], header[i + 1]]);
        sum = sum.wrapping_add(word as u32);
        i += 2;
    }

    if i < header.len() {
        let word = u16::from_be_bytes([header[i], 0]);
        sum = sum.wrapping_add(word as u32);
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !(sum as u16)
}

/// Verifies an IPv4 header checksum according to RFC 791.
pub fn verify_ipv4_checksum(header: &[u8]) -> bool {
    let mut sum: u32 = 0;
    let mut i = 0;

    while i + 1 < header.len() {
        let word = u16::from_be_bytes([header[i], header[i + 1]]);
        sum = sum.wrapping_add(word as u32);
        i += 2;
    }

    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    (sum as u16) == 0xFFFF
}

/// Parsed IPv4 Packet
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ipv4Packet<'a> {
    pub src: Ipv4Address,
    pub dst: Ipv4Address,
    pub protocol: IpProtocol,
    pub ttl: u8,
    pub payload: &'a [u8],
}

impl<'a> Ipv4Packet<'a> {
    /// Parses an IPv4 packet, validating header length and RFC 791 checksum.
    pub fn parse(buf: &'a [u8]) -> Result<Self, &'static str> {
        if buf.len() < IPV4_MIN_HEADER_LEN {
            return Err("Buffer too small for IPv4 header");
        }

        let ver_ihl = buf[0];
        let version = (ver_ihl >> 4) & 0x0F;
        if version != 4 {
            return Err("Invalid IP version (expected IPv4)");
        }

        let ihl = (ver_ihl & 0x0F) as usize * 4;
        if ihl < IPV4_MIN_HEADER_LEN || buf.len() < ihl {
            return Err("Invalid IHL header length");
        }

        if !verify_ipv4_checksum(&buf[0..ihl]) {
            return Err("IPv4 header checksum verification failed");
        }

        let total_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
        if buf.len() < total_len || total_len < ihl {
            return Err("Invalid IPv4 total packet length");
        }

        let ttl = buf[8];
        let protocol = IpProtocol::from(buf[9]);
        let src = Ipv4Address([buf[12], buf[13], buf[14], buf[15]]);
        let dst = Ipv4Address([buf[16], buf[17], buf[18], buf[19]]);

        Ok(Ipv4Packet {
            src,
            dst,
            protocol,
            ttl,
            payload: &buf[ihl..total_len],
        })
    }

    /// Serializes an IPv4 packet with automatically calculated checksum into a buffer.
    pub fn serialize(
        src: Ipv4Address,
        dst: Ipv4Address,
        protocol: IpProtocol,
        ttl: u8,
        payload: &[u8],
        out: &mut [u8],
    ) -> Result<usize, &'static str> {
        let total_len = IPV4_MIN_HEADER_LEN + payload.len();
        if out.len() < total_len {
            return Err("Output buffer too small for IPv4 serialization");
        }

        out[0] = 0x45; // Version 4, IHL 5 (20 bytes)
        out[1] = 0x00; // DSCP / ECN
        out[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        out[4..6].copy_from_slice(&0u16.to_be_bytes()); // Identification
        out[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Don't Fragment
        out[8] = ttl;
        out[9] = u8::from(protocol);
        out[10..12].copy_from_slice(&0u16.to_be_bytes()); // Zero checksum before calculation
        out[12..16].copy_from_slice(&src.0);
        out[16..20].copy_from_slice(&dst.0);

        let checksum = calculate_ipv4_checksum(&out[0..IPV4_MIN_HEADER_LEN]);
        out[10..12].copy_from_slice(&checksum.to_be_bytes());

        out[IPV4_MIN_HEADER_LEN..total_len].copy_from_slice(payload);
        Ok(total_len)
    }
}
