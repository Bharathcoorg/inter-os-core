// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! UDP (User Datagram Protocol) Datagram Layer
//!
//! Complies with RFC 768. Provides connectionless datagram transport.

pub const UDP_HEADER_LEN: usize = 8;

/// Parsed UDP Datagram View
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpDatagram<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub length: u16,
    pub checksum: u16,
    pub payload: &'a [u8],
}

impl<'a> UdpDatagram<'a> {
    /// Parses a UDP datagram from a byte buffer.
    pub fn parse(buf: &'a [u8]) -> Result<Self, &'static str> {
        if buf.len() < UDP_HEADER_LEN {
            return Err("Buffer too small for UDP header");
        }

        let src_port = u16::from_be_bytes([buf[0], buf[1]]);
        let dst_port = u16::from_be_bytes([buf[2], buf[3]]);
        let length = u16::from_be_bytes([buf[4], buf[5]]) as usize;
        let checksum = u16::from_be_bytes([buf[6], buf[7]]);

        if dst_port == 0 {
            return Err("Invalid UDP destination port (zero)");
        }

        if buf.len() < length || length < UDP_HEADER_LEN {
            return Err("Invalid UDP datagram length field");
        }

        Ok(UdpDatagram {
            src_port,
            dst_port,
            length: length as u16,
            checksum,
            payload: &buf[UDP_HEADER_LEN..length],
        })
    }

    /// Serializes a UDP datagram into an output buffer.
    pub fn serialize(
        src_port: u16,
        dst_port: u16,
        payload: &[u8],
        out: &mut [u8],
    ) -> Result<usize, &'static str> {
        let total_len = UDP_HEADER_LEN + payload.len();
        if out.len() < total_len {
            return Err("Output buffer too small for UDP serialization");
        }

        out[0..2].copy_from_slice(&src_port.to_be_bytes());
        out[2..4].copy_from_slice(&dst_port.to_be_bytes());
        out[4..6].copy_from_slice(&(total_len as u16).to_be_bytes());
        out[6..8].copy_from_slice(&0u16.to_be_bytes()); // Checksum optional in IPv4
        out[UDP_HEADER_LEN..total_len].copy_from_slice(payload);

        Ok(total_len)
    }
}

/// Verifies RFC 768 UDP checksum against IPv4 pseudo-header.
/// If transmitted checksum is 0x0000, checksum verification is skipped (optional in IPv4).
pub fn verify_udp_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], udp_raw: &[u8]) -> bool {
    if udp_raw.len() < UDP_HEADER_LEN {
        return false;
    }
    let transmitted_csum = u16::from_be_bytes([udp_raw[6], udp_raw[7]]);
    if transmitted_csum == 0 {
        return true; // Optional in IPv4 RFC 768
    }
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&src_ip);
    pseudo[4..8].copy_from_slice(&dst_ip);
    pseudo[8] = 0;
    pseudo[9] = 17; // IP protocol UDP
    pseudo[10..12].copy_from_slice(&(udp_raw.len() as u16).to_be_bytes());

    let mut sum: u32 = 0;
    for chunk in pseudo.chunks(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    for chunk in udp_raw.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += word as u32;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let computed = !sum as u16;
    computed == 0
}
