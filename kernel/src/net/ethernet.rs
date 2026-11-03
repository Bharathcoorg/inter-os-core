// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! IEEE 802.3 Ethernet II Frame Framing & MAC Layer
//!
//! Pure no_std Ethernet frame parser, serializer, and MAC address representations.

use core::fmt;

/// 48-bit Media Access Control (MAC) Address
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    pub const BROADCAST: MacAddress = MacAddress([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    pub const ZERO: MacAddress = MacAddress([0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    pub const fn new(bytes: [u8; 6]) -> Self {
        MacAddress(bytes)
    }

    pub fn is_broadcast(&self) -> bool {
        self.0 == Self::BROADCAST.0
    }
}

impl fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Standard Ethernet Protocol Identifiers (EtherType)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
    Ipv4 = 0x0800,
    Arp = 0x0806,
    Ipv6 = 0x86DD,
    Unknown(u16),
}

impl From<u16> for EtherType {
    fn from(val: u16) -> Self {
        match val {
            0x0800 => EtherType::Ipv4,
            0x0806 => EtherType::Arp,
            0x86DD => EtherType::Ipv6,
            other => EtherType::Unknown(other),
        }
    }
}

impl From<EtherType> for u16 {
    fn from(ethertype: EtherType) -> Self {
        match ethertype {
            EtherType::Ipv4 => 0x0800,
            EtherType::Arp => 0x0806,
            EtherType::Ipv6 => 0x86DD,
            EtherType::Unknown(v) => v,
        }
    }
}

pub const ETHERNET_HEADER_LEN: usize = 14;

/// IEEE 802.3 Ethernet II Frame View
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EthernetFrame<'a> {
    pub dst_mac: MacAddress,
    pub src_mac: MacAddress,
    pub ethertype: EtherType,
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    /// Parses an Ethernet II frame from a raw byte buffer.
    pub fn parse(buf: &'a [u8]) -> Result<Self, &'static str> {
        if buf.len() < ETHERNET_HEADER_LEN {
            return Err("Buffer too small for Ethernet header");
        }

        let mut dst = [0u8; 6];
        let mut src = [0u8; 6];
        dst.copy_from_slice(&buf[0..6]);
        src.copy_from_slice(&buf[6..12]);

        let ethertype_raw = u16::from_be_bytes([buf[12], buf[13]]);

        Ok(EthernetFrame {
            dst_mac: MacAddress(dst),
            src_mac: MacAddress(src),
            ethertype: EtherType::from(ethertype_raw),
            payload: &buf[ETHERNET_HEADER_LEN..],
        })
    }

    /// Serializes the frame header and payload into a target destination buffer.
    pub fn serialize(&self, dst: &mut [u8]) -> Result<usize, &'static str> {
        let total_len = ETHERNET_HEADER_LEN + self.payload.len();
        if dst.len() < total_len {
            return Err("Target buffer too small for Ethernet serialization");
        }

        dst[0..6].copy_from_slice(&self.dst_mac.0);
        dst[6..12].copy_from_slice(&self.src_mac.0);
        let ethertype_bytes = u16::from(self.ethertype).to_be_bytes();
        dst[12..14].copy_from_slice(&ethertype_bytes);
        dst[ETHERNET_HEADER_LEN..total_len].copy_from_slice(self.payload);

        Ok(total_len)
    }
}
