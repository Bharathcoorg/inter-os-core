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
