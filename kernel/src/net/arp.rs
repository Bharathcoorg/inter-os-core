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

        let opcode_raw = u16::from_be_bytes([buf[6], buf[7]]);
        if opcode_raw != 1 && opcode_raw != 2 {
            return Err("Unsupported ARP opcode");
        }
        let opcode = ArpOpcode::from(opcode_raw);

        let mut smac = [0u8; 6];
        smac.copy_from_slice(&buf[8..14]);
        if smac[0] & 1 != 0 || smac == [0; 6] {
            return Err("ARP sender MAC cannot be multicast or broadcast");
        }

        let sip = Ipv4Address([buf[14], buf[15], buf[16], buf[17]]);
        if sip.0[0] == 0 || sip.0[0] >= 224 || sip.0 == [255, 255, 255, 255] {
            return Err("ARP sender IP cannot be zero, multicast, or broadcast");
        }

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
pub struct ArpEntry {
    pub ip: Ipv4Address,
    pub mac: MacAddress,
    pub valid: bool,
    pub is_permanent: bool,
    pub expires_at: u64,
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
                is_permanent: false,
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

    /// Inserts or pins an immutable permanent ARP entry (e.g. default gateway or static binding).
    /// Permanent entries can NEVER be evicted by dynamic ARP cache saturation,
    /// nor can they be downgraded or overwritten by untrusted wire ARP packets.
    pub fn insert_permanent(&mut self, ip: Ipv4Address, mac: MacAddress) {
        // If entry exists, mark permanent and update MAC
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                if entry.is_permanent {
                    // Already permanent: immutable, cannot be overwritten or altered!
                    return;
                }
                entry.mac = mac;
                entry.is_permanent = true;
                entry.expires_at = u64::MAX;
                return;
            }
        }

        // Reuse an invalid or non-permanent expired slot
        for entry in self.entries.iter_mut() {
            if !entry.valid || (!entry.is_permanent && entry.expires_at == 0) {
                if !entry.valid {
                    self.count += 1;
                }
                entry.ip = ip;
                entry.mac = mac;
                entry.valid = true;
                entry.is_permanent = true;
                entry.expires_at = u64::MAX;
                return;
            }
        }

        // Evict dynamic entry with closest expiry
        let mut oldest_idx = None;
        let mut min_expiry = u64::MAX;
        for (i, entry) in self.entries.iter().enumerate() {
            if !entry.is_permanent && entry.expires_at < min_expiry {
                min_expiry = entry.expires_at;
                oldest_idx = Some(i);
            }
        }

        if let Some(idx) = oldest_idx {
            self.entries[idx] = ArpEntry {
                ip,
                mac,
                valid: true,
                is_permanent: true,
                expires_at: u64::MAX,
            };
        }
    }

    /// Returns whether the specified IP has a permanent/immutable ARP binding.
    pub fn is_permanent(&self, ip: Ipv4Address) -> bool {
        self.entries.iter().any(|e| e.valid && e.ip == ip && e.is_permanent)
    }

    /// Inserts or refreshes a dynamic ARP mapping with a bounded lifetime.
    pub fn insert_at(&mut self, ip: Ipv4Address, mac: MacAddress, now: u64) {
        self.insert_with_expiry(ip, mac, now, now.saturating_add(ARP_CACHE_TICKS));
    }

    fn insert_with_expiry(&mut self, ip: Ipv4Address, mac: MacAddress, now: u64, expires_at: u64) {
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.ip == ip {
                // RFC Security: Dynamic wire packets MUST NOT overwrite or downgrade permanent entries!
                if entry.is_permanent {
                    return;
                }
                entry.mac = mac;
                entry.expires_at = expires_at;
                return;
            }
        }

        // Reuse an invalid or expired non-permanent entry before consuming a genuinely free slot.
        for entry in self.entries.iter_mut() {
            if !entry.valid || (!entry.is_permanent && entry.expires_at <= now) {
                if !entry.valid {
                    self.count += 1;
                }
                entry.ip = ip;
                entry.mac = mac;
                entry.valid = true;
                entry.is_permanent = false;
                entry.expires_at = expires_at;
                return;
            }
        }

        // Evict non-permanent entry closest to expiry if full (never evict permanent entries).
        let mut oldest_idx = None;
        let mut min_expiry = u64::MAX;
        for (i, entry) in self.entries.iter().enumerate() {
            if !entry.is_permanent && entry.expires_at < min_expiry {
                min_expiry = entry.expires_at;
                oldest_idx = Some(i);
            }
        }
        if let Some(idx) = oldest_idx {
            self.entries[idx] = ArpEntry {
                ip,
                mac,
                valid: true,
                is_permanent: false,
                expires_at,
            };
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arp_roundtrip_and_parse() {
        let req = ArpPacket {
            opcode: ArpOpcode::Request,
            sender_mac: MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
            sender_ip: Ipv4Address([192, 168, 1, 100]),
            target_mac: MacAddress([0; 6]),
            target_ip: Ipv4Address([192, 168, 1, 1]),
        };
        let mut buf = [0u8; 32];
        let n = req.serialize(&mut buf).unwrap();
        assert_eq!(n, ARP_PACKET_LEN);

        let parsed = ArpPacket::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.opcode, ArpOpcode::Request);
        assert_eq!(parsed.sender_mac, req.sender_mac);
        assert_eq!(parsed.sender_ip, req.sender_ip);
        assert_eq!(parsed.target_mac, req.target_mac);
        assert_eq!(parsed.target_ip, req.target_ip);
    }

    #[test]
    fn arp_rejects_illegal_opcodes_mac_and_ip() {
        let req = ArpPacket {
            opcode: ArpOpcode::Request,
            sender_mac: MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
            sender_ip: Ipv4Address([192, 168, 1, 100]),
            target_mac: MacAddress([0; 6]),
            target_ip: Ipv4Address([192, 168, 1, 1]),
        };
        let mut buf = [0u8; 32];
        let n = req.serialize(&mut buf).unwrap();

        // 1. Illegal opcode (e.g. 3, RARP)
        let mut bad_buf = buf;
        bad_buf[6..8].copy_from_slice(&3u16.to_be_bytes());
        assert_eq!(ArpPacket::parse(&bad_buf[..n]), Err("Unsupported ARP opcode"));

        // 2. Multicast sender MAC (LSB of first byte is 1)
        let mut mcast_mac_buf = buf;
        mcast_mac_buf[8] = 0x01;
        assert_eq!(
            ArpPacket::parse(&mcast_mac_buf[..n]),
            Err("ARP sender MAC cannot be multicast or broadcast")
        );

        // 3. All-zero sender MAC
        let mut zero_mac_buf = buf;
        zero_mac_buf[8..14].copy_from_slice(&[0; 6]);
        assert_eq!(
            ArpPacket::parse(&zero_mac_buf[..n]),
            Err("ARP sender MAC cannot be multicast or broadcast")
        );

        // 4. Zero sender IP
        let mut zero_ip_buf = buf;
        zero_ip_buf[14..18].copy_from_slice(&[0; 4]);
        assert_eq!(
            ArpPacket::parse(&zero_ip_buf[..n]),
            Err("ARP sender IP cannot be zero, multicast, or broadcast")
        );

        // 5. Multicast sender IP
        let mut mcast_ip_buf = buf;
        mcast_ip_buf[14..18].copy_from_slice(&[224, 0, 0, 1]);
        assert_eq!(
            ArpPacket::parse(&mcast_ip_buf[..n]),
            Err("ARP sender IP cannot be zero, multicast, or broadcast")
        );

        // 6. Broadcast sender IP
        let mut bcast_ip_buf = buf;
        bcast_ip_buf[14..18].copy_from_slice(&[255, 255, 255, 255]);
        assert_eq!(
            ArpPacket::parse(&bcast_ip_buf[..n]),
            Err("ARP sender IP cannot be zero, multicast, or broadcast")
        );
    }

    #[test]
    fn arp_table_evicts_closest_to_expiry_not_slot_zero() {
        let mut table = ArpTable::new();
        let gateway_ip = Ipv4Address([192, 168, 1, 1]);
        let gateway_mac = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);

        // Slot 0: Gateway with large expiration time
        table.insert_with_expiry(gateway_ip, gateway_mac, 0, 100_000);

        // Fill remaining 15 slots with expiration time 50_000, except slot 5 with 10 (soonest to expire)
        for i in 2..=16 {
            let ip = Ipv4Address([192, 168, 1, i as u8]);
            let mac = MacAddress([0x00, 0x11, 0x22, 0x33, 0x44, i as u8]);
            let expiry = if i == 6 { 10 } else { 50_000 };
            table.insert_with_expiry(ip, mac, 0, expiry);
        }
        assert_eq!(table.len(), MAX_ARP_ENTRIES);

        // Now insert 17th entry. Table is saturated.
        let new_ip = Ipv4Address([192, 168, 1, 200]);
        let new_mac = MacAddress([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
        table.insert_with_expiry(new_ip, new_mac, 0, 60_000);

        // Verify gateway in slot 0 was NOT evicted!
        assert_eq!(table.lookup(gateway_ip), Some(gateway_mac));

        // Verify the soonest-to-expire entry (192.168.1.6) WAS evicted!
        assert_eq!(table.lookup(Ipv4Address([192, 168, 1, 6])), None);

        // Verify new entry was inserted!
        assert_eq!(table.lookup(new_ip), Some(new_mac));
    }

    #[test]
    fn permanent_gateway_cannot_be_evicted_or_downgraded() {
        let mut table = ArpTable::new();
        let gateway_ip = Ipv4Address([10, 0, 2, 2]);
        let gateway_mac = MacAddress([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

        table.insert_permanent(gateway_ip, gateway_mac);
        assert!(table.is_permanent(gateway_ip));
        assert_eq!(table.lookup(gateway_ip), Some(gateway_mac));

        // 1. Attempt to downgrade / spoof via dynamic wire packet: MUST BE IGNORED
        let fake_mac = MacAddress([0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]);
        table.insert_at(gateway_ip, fake_mac, 100);
        assert_eq!(table.lookup(gateway_ip), Some(gateway_mac)); // Unchanged!
        assert!(table.is_permanent(gateway_ip));

        // 2. Flood 30 dynamic entries with much larger expiration timestamps
        for i in 10..=40 {
            let ip = Ipv4Address([10, 0, 2, i as u8]);
            let mac = MacAddress([0x00, 0x22, 0x33, 0x44, 0x55, i as u8]);
            table.insert_at(ip, mac, 1000 + (i as u64) * 10);
        }

        // Verify Gateway was NEVER evicted despite extreme cache saturation
        assert_eq!(table.lookup(gateway_ip), Some(gateway_mac));
        assert!(table.is_permanent(gateway_ip));
    }
}
