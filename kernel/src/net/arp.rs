// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Address Resolution Protocol (ARP) & In-Kernel ARP Cache
//!
//! Complies with RFC 826. Translates IPv4 addresses to physical MAC hardware addresses.

use super::ethernet::MacAddress;
use super::ipv4::Ipv4Address;

pub const ARP_PACKET_LEN: usize = 28;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ArpOpcode {
    Request = 1,
    Reply = 2,
    Unknown(u16),
}

impl From<u16> for ArpOpcode {
    fn from(val: u16) -> Self {
        match val {
            1 => ArpOpcode::Request,
            2 => ArpOpcode::Reply,
            other => ArpOpcode::Unknown(other),
        }
    }
}

/// RFC 826 ARP Packet Representation
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArpPacket {
    pub opcode: ArpOpcode,
    pub sender_mac: MacAddress,
    pub sender_ip: Ipv4Address,
    pub target_mac: MacAddress,
    pub target_ip: Ipv4Address,
}

impl ArpPacket {
    /// Parses an ARP packet from a byte buffer.
    pub fn parse(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < ARP_PACKET_LEN {
            return Err("Buffer too small for ARP packet");
        }

        let htype = u16::from_be_bytes([buf[0], buf[1]]);
        let ptype = u16::from_be_bytes([buf[2], buf[3]]);
        let hlen = buf[4];
        let plen = buf[5];

        if htype != 1 || ptype != 0x0800 || hlen != 6 || plen != 4 {
            return Err("Unsupported ARP hardware/protocol parameters (expected Ethernet/IPv4)");
        }

        let opcode = ArpOpcode::from(u16::from_be_bytes([buf[6], buf[7]]));

        let mut smac = [0u8; 6];
        smac.copy_from_slice(&buf[8..14]);
        let sip = Ipv4Address([buf[14], buf[15], buf[16], buf[17]]);

        let mut tmac = [0u8; 6];
        tmac.copy_from_slice(&buf[18..24]);
        let tip = Ipv4Address([buf[24], buf[25], buf[26], buf[27]]);

        Ok(ArpPacket {
            opcode,
            sender_mac: MacAddress(smac),
            sender_ip: sip,
            target_mac: MacAddress(tmac),
            target_ip: tip,
        })
    }

    /// Serializes an ARP packet into an output byte buffer.
    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if out.len() < ARP_PACKET_LEN {
            return Err("Buffer too small for ARP packet serialization");
        }

        out[0..2].copy_from_slice(&1u16.to_be_bytes()); // Hardware Type: Ethernet (1)
        out[2..4].copy_from_slice(&0x0800u16.to_be_bytes()); // Protocol Type: IPv4 (0x0800)
        out[4] = 6; // Hardware address size
        out[5] = 4; // Protocol address size

        let opcode_val = match self.opcode {
            ArpOpcode::Request => 1u16,
            ArpOpcode::Reply => 2u16,
            ArpOpcode::Unknown(v) => v,
        };
        out[6..8].copy_from_slice(&opcode_val.to_be_bytes());
        out[8..14].copy_from_slice(&self.sender_mac.0);
        out[14..18].copy_from_slice(&self.sender_ip.0);
        out[18..24].copy_from_slice(&self.target_mac.0);
        out[24..28].copy_from_slice(&self.target_ip.0);

        Ok(ARP_PACKET_LEN)
    }
}

pub const MAX_ARP_ENTRIES: usize = 16;
pub const ARP_CACHE_TICKS: u64 = 600_000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ArpEntry {
    ip: Ipv4Address,
    mac: MacAddress,
    valid: bool,
    expires_at: u64,
}

/// In-Kernel Static/Dynamic ARP Resolution Table
pub struct ArpTable {
    entries: [ArpEntry; MAX_ARP_ENTRIES],
    count: usize,
}

impl ArpTable {
    pub const fn new() -> Self {
        Self {
            entries: [ArpEntry {
                ip: Ipv4Address::UNSPECIFIED,
                mac: MacAddress::ZERO,
                valid: false,
                expires_at: 0,
            }; MAX_ARP_ENTRIES],
            count: 0,
        }
    }

    /// Inserts a mapping with no expiration. Used by deterministic kernel
    /// self-tests and retained for compatibility with the original API.
    pub fn insert(&mut self, ip: Ipv4Address, mac: MacAddress) {
        self.insert_with_expiry(ip, mac, 0, u64::MAX);
    }

    /// Inserts or refreshes a dynamic ARP mapping with a bounded lifetime.
    pub fn insert_at(&mut self, ip: Ipv4Address, mac: MacAddress, now: u64) {
        self.insert_with_expiry(ip, mac, now, now.saturating_add(ARP_CACHE_TICKS));
    }

    fn insert_with_expiry(&mut self, ip: Ipv4Address, mac: MacAddress, now: u64, expires_at: u64) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                entry.mac = mac;
                entry.expires_at = expires_at;
                return;
            }
        }

        // Reuse an expired entry before consuming a genuinely free slot.
        for entry in self.entries.iter_mut() {
            if !entry.valid || entry.expires_at <= now {
                if !entry.valid {
                    self.count += 1;
                }
                entry.ip = ip;
                entry.mac = mac;
                entry.valid = true;
                entry.expires_at = expires_at;
                return;
            }
        }

        // Evict slot 0 if full.
        self.entries[0] = ArpEntry { ip, mac, valid: true, expires_at };
    }

    /// Looks up the hardware MAC address for a given IPv4 address.
    pub fn lookup(&self, ip: Ipv4Address) -> Option<MacAddress> {
        self.entries.iter()
            .find(|entry| entry.valid && entry.ip == ip)
            .map(|entry| entry.mac)
    }

    /// Looks up only a live dynamic mapping at the supplied tick.
    pub fn lookup_at(&self, ip: Ipv4Address, now: u64) -> Option<MacAddress> {
        for entry in &self.entries {
            if entry.valid && entry.ip == ip && entry.expires_at > now {
                return Some(entry.mac);
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.count
    }
}
