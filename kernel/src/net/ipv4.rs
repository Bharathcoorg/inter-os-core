// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

        let frag_flags_offset = u16::from_be_bytes([buf[6], buf[7]]);
        if (frag_flags_offset & 0x8000) != 0 {
            return Err("IPv4 reserved flag bit set; packet dropped");
        }
        if (frag_flags_offset & 0x3FFF) != 0 {
            return Err("IPv4 fragments not supported; packet dropped");
        }

        let ttl = buf[8];
        let protocol = IpProtocol::from(buf[9]);
        let src = Ipv4Address([buf[12], buf[13], buf[14], buf[15]]);
        let dst = Ipv4Address([buf[16], buf[17], buf[18], buf[19]]);

        if src.0[0] >= 224 || src.0 == [255, 255, 255, 255] {
            return Err("Invalid IPv4 source address (multicast or broadcast)");
        }

        if src == Ipv4Address::UNSPECIFIED {
            return Err("Invalid IPv4 source address (zero/unspecified)");
        }

        if dst == Ipv4Address::UNSPECIFIED {
            return Err("Invalid IPv4 destination address (zero/unspecified)");
        }

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
        if payload.len() > 65535 - IPV4_MIN_HEADER_LEN {
            return Err("IPv4 payload exceeds maximum datagram size");
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_roundtrip_and_checksum_verification() {
        let src = Ipv4Address([192, 168, 1, 10]);
        let dst = Ipv4Address([192, 168, 1, 1]);
        let payload = b"Hello, InterOS IPv4!";
        let mut buf = [0u8; 128];

        let len = Ipv4Packet::serialize(src, dst, IpProtocol::Udp, 64, payload, &mut buf)
            .expect("serialization succeeds");
        assert_eq!(len, IPV4_MIN_HEADER_LEN + payload.len());

        let parsed = Ipv4Packet::parse(&buf[..len]).expect("parsing succeeds");
        assert_eq!(parsed.src, src);
        assert_eq!(parsed.dst, dst);
        assert_eq!(parsed.protocol, IpProtocol::Udp);
        assert_eq!(parsed.ttl, 64);
        assert_eq!(parsed.payload, payload);

        // Header checksum of a valid serialized packet must verify to 0
        assert_eq!(calculate_ipv4_checksum(&buf[..IPV4_MIN_HEADER_LEN]), 0);
    }

    #[test]
    fn ipv4_fragment_rejection() {
        let src = Ipv4Address([10, 0, 0, 1]);
        let dst = Ipv4Address([10, 0, 0, 2]);
        let mut buf = [0u8; 64];
        let len = Ipv4Packet::serialize(src, dst, IpProtocol::Tcp, 64, b"data", &mut buf).unwrap();

        // 1. More Fragments (MF) bit set (bit 13 / 0x2000 in flags+offset)
        let mut mf_buf = buf;
        mf_buf[6] |= 0x20;
        mf_buf[10..12].copy_from_slice(&[0, 0]);
        let csum = calculate_ipv4_checksum(&mf_buf[..IPV4_MIN_HEADER_LEN]);
        mf_buf[10..12].copy_from_slice(&csum.to_be_bytes());
        assert_eq!(
            Ipv4Packet::parse(&mf_buf[..len]),
            Err("IPv4 fragments not supported; packet dropped")
        );

        // 2. Non-zero fragment offset (e.g. offset = 1)
        let mut frag_buf = buf;
        frag_buf[7] |= 0x01;
        frag_buf[10..12].copy_from_slice(&[0, 0]);
        let csum = calculate_ipv4_checksum(&frag_buf[..IPV4_MIN_HEADER_LEN]);
        frag_buf[10..12].copy_from_slice(&csum.to_be_bytes());
        assert_eq!(
            Ipv4Packet::parse(&frag_buf[..len]),
            Err("IPv4 fragments not supported; packet dropped")
        );
    }

    #[test]
    fn ipv4_multicast_broadcast_source_rejection() {
        let mut buf = [0u8; 64];
        let len = Ipv4Packet::serialize(
            Ipv4Address([10, 0, 0, 1]),
            Ipv4Address([10, 0, 0, 2]),
            IpProtocol::Udp,
            64,
            b"test",
            &mut buf,
        ).unwrap();

        // 1. Multicast source (224.0.0.1)
        let mut mcast_buf = buf;
        mcast_buf[12..16].copy_from_slice(&[224, 0, 0, 1]);
        mcast_buf[10..12].copy_from_slice(&[0, 0]);
        let csum = calculate_ipv4_checksum(&mcast_buf[..IPV4_MIN_HEADER_LEN]);
        mcast_buf[10..12].copy_from_slice(&csum.to_be_bytes());
        assert_eq!(
            Ipv4Packet::parse(&mcast_buf[..len]),
            Err("Invalid IPv4 source address (multicast or broadcast)")
        );

        // 2. Broadcast source (255.255.255.255)
        let mut bcast_buf = buf;
        bcast_buf[12..16].copy_from_slice(&[255, 255, 255, 255]);
        bcast_buf[10..12].copy_from_slice(&[0, 0]);
        let csum = calculate_ipv4_checksum(&bcast_buf[..IPV4_MIN_HEADER_LEN]);
        bcast_buf[10..12].copy_from_slice(&csum.to_be_bytes());
        assert_eq!(
            Ipv4Packet::parse(&bcast_buf[..len]),
            Err("Invalid IPv4 source address (multicast or broadcast)")
        );
    }
}
