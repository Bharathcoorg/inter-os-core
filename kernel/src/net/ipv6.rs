// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bounded IPv6 packet primitives for the InterOS network stack.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ipv6Address(pub [u8; 16]);

impl Ipv6Address {
    pub const LOCALHOST: Self = Self([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    pub const UNSPECIFIED: Self = Self([0; 16]);
    pub const ALL_NODES: Self = Self([0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

    #[inline]
    pub fn is_multicast(&self) -> bool {
        self.0[0] == 0xff
    }

    #[inline]
    pub fn is_unspecified(&self) -> bool {
        self.0 == [0u8; 16]
    }

    #[inline]
    pub fn is_link_local(&self) -> bool {
        self.0[0] == 0xfe && (self.0[1] & 0xc0) == 0x80
    }

    #[inline]
    pub fn is_loopback(&self) -> bool {
        self.0 == Self::LOCALHOST.0
    }

    #[inline]
    pub fn solicited_node_multicast(&self) -> Ipv6Address {
        let mut bytes = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0xff, 0, 0, 0];
        bytes[13] = self.0[13];
        bytes[14] = self.0[14];
        bytes[15] = self.0[15];
        Ipv6Address(bytes)
    }

    #[inline]
    pub fn multicast_mac(&self) -> crate::net::ethernet::MacAddress {
        crate::net::ethernet::MacAddress([
            0x33,
            0x33,
            self.0[12],
            self.0[13],
            self.0[14],
            self.0[15],
        ])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Ipv6NextHeader {
    HopByHop = 0,
    Tcp = 6,
    Udp = 17,
    Icmpv6 = 58,
    NoNext = 59,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ipv6Packet<'a> {
    pub src: Ipv6Address,
    pub dst: Ipv6Address,
    pub next_header: u8,
    pub hop_limit: u8,
    pub payload: &'a [u8],
}

impl<'a> Ipv6Packet<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 40 {
            return Err("IPv6 header too short");
        }
        if data[0] >> 4 != 6 {
            return Err("invalid IPv6 version");
        }
        let payload_len = u16::from_be_bytes([data[4], data[5]]) as usize;
        if data.len() < 40 + payload_len {
            return Err("IPv6 payload length mismatch");
        }
        let mut src = [0u8; 16];
        src.copy_from_slice(&data[8..24]);
        let mut dst = [0u8; 16];
        dst.copy_from_slice(&data[24..40]);
        Ok(Self {
            src: Ipv6Address(src),
            dst: Ipv6Address(dst),
            next_header: data[6],
            hop_limit: data[7],
            payload: &data[40..40 + payload_len],
        })
    }

    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if self.payload.len() > u16::MAX as usize || out.len() < 40 + self.payload.len() {
            return Err("IPv6 packet too large");
        }
        out[..40 + self.payload.len()].fill(0);
        out[0] = 0x60;
        let len = (self.payload.len() as u16).to_be_bytes();
        out[4..6].copy_from_slice(&len);
        out[6] = self.next_header;
        out[7] = self.hop_limit;
        out[8..24].copy_from_slice(&self.src.0);
        out[24..40].copy_from_slice(&self.dst.0);
        out[40..40 + self.payload.len()].copy_from_slice(self.payload);
        Ok(40 + self.payload.len())
    }
}

/// RFC 4443 & RFC 8200 IPv6 Pseudo-Header Checksum Engine
pub fn calculate_icmpv6_checksum(src: &Ipv6Address, dst: &Ipv6Address, payload: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    // 1. Source IPv6 Address (16 bytes)
    for chunk in src.0.chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    // 2. Destination IPv6 Address (16 bytes)
    for chunk in dst.0.chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    // 3. Payload length (32 bits)
    let len = payload.len() as u32;
    sum += len >> 16;
    sum += len & 0xffff;
    // 4. Next header: 58 (ICMPv6)
    sum += 58u32;
    // 5. ICMPv6 Payload
    let mut chunks = payload.chunks_exact(2);
    for chunk in chunks.by_ref() {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&rem) = chunks.remainder().first() {
        sum += (rem as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let res = !(sum as u16);
    if res == 0 { 0xffff } else { res }
}

/// Verifies that received ICMPv6 message matches RFC 4443 pseudo-header checksum.
pub fn verify_icmpv6_checksum(src: &Ipv6Address, dst: &Ipv6Address, payload: &[u8]) -> bool {
    if payload.len() < 4 {
        return false;
    }
    let mut sum: u32 = 0;
    for chunk in src.0.chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    for chunk in dst.0.chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    let len = payload.len() as u32;
    sum += len >> 16;
    sum += len & 0xffff;
    sum += 58u32;
    let mut chunks = payload.chunks_exact(2);
    for chunk in chunks.by_ref() {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&rem) = chunks.remainder().first() {
        sum += (rem as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    (sum as u16) == 0xffff
}

/// Verifies that received NDP packet has Hop Limit == 255 (RFC 4861 § 6.1.1, 6.1.2, 7.1.1, 7.1.2)
#[inline]
pub fn verify_ndp_hop_limit(hop_limit: u8) -> bool {
    hop_limit == 255
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Prefix {
    pub address: Ipv6Address,
    pub prefix_len: u8,
}

impl Ipv6Prefix {
    pub fn new(mut address: Ipv6Address, prefix_len: u8) -> Self {
        let len = prefix_len.min(128) as usize;
        let full = len / 8;
        let rem = len % 8;
        if full < 16 {
            if rem > 0 {
                let mask = 0xffu8 << (8 - rem);
                address.0[full] &= mask;
                for b in &mut address.0[full + 1..16] {
                    *b = 0;
                }
            } else {
                for b in &mut address.0[full..16] {
                    *b = 0;
                }
            }
        }
        Self { address, prefix_len }
    }

    pub fn contains(&self, ip: Ipv6Address) -> bool {
        if self.prefix_len > 128 {
            return false;
        }
        let full = (self.prefix_len / 8) as usize;
        let rem = self.prefix_len % 8;
        if self.address.0[..full] != ip.0[..full] {
            return false;
        }
        if rem == 0 {
            return true;
        }
        let mask = 0xffu8 << (8 - rem);
        (self.address.0[full] & mask) == (ip.0[full] & mask)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Route {
    pub prefix: Ipv6Prefix,
    pub next_hop: Option<Ipv6Address>,
    pub interface: u8,
    pub metric: u16,
}

pub const MAX_IPV6_ROUTES: usize = 32;
pub struct Ipv6RoutingTable {
    routes: [Option<Ipv6Route>; MAX_IPV6_ROUTES],
}

impl Ipv6RoutingTable {
    pub const fn new() -> Self {
        Self {
            routes: [None; MAX_IPV6_ROUTES],
        }
    }

    pub fn add(&mut self, r: Ipv6Route) -> Result<(), Ipv6Route> {
        if r.prefix.prefix_len > 128 {
            return Err(r);
        }
        // In-place update if prefix already exists (deduplication prevents route exhaustion)
        for entry in self.routes.iter_mut().flatten() {
            if entry.prefix == r.prefix {
                *entry = r;
                return Ok(());
            }
        }
        if let Some(s) = self.routes.iter_mut().find(|x| x.is_none()) {
            *s = Some(r);
            Ok(())
        } else {
            Err(r)
        }
    }

    pub fn remove_prefix(&mut self, prefix: Ipv6Prefix) -> bool {
        for entry in &mut self.routes {
            if entry.map(|r| r.prefix == prefix).unwrap_or(false) {
                *entry = None;
                return true;
            }
        }
        false
    }

    pub fn remove_next_hop(&mut self, next_hop: &Ipv6Address) -> bool {
        let mut removed = false;
        for entry in &mut self.routes {
            if entry.map(|r| r.next_hop.as_ref() == Some(next_hop)).unwrap_or(false) {
                *entry = None;
                removed = true;
            }
        }
        removed
    }

    pub fn remove_interface(&mut self, interface: u8) {
        for entry in &mut self.routes {
            if entry.map(|r| r.interface == interface).unwrap_or(false) {
                *entry = None;
            }
        }
    }

    pub fn lookup(&self, dst: Ipv6Address) -> Option<Ipv6Route> {
        self.routes
            .iter()
            .flatten()
            .filter(|r| r.prefix.contains(dst))
            .max_by_key(|r| (r.prefix.prefix_len as u16, u16::MAX - r.metric))
            .copied()
    }

    pub fn len(&self) -> usize {
        self.routes.iter().filter(|r| r.is_some()).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeighborEntry {
    pub address: Ipv6Address,
    pub mac: [u8; 6],
    pub is_router: bool,
    pub reachable_until: u64,
}

pub const MAX_IPV6_NEIGHBORS: usize = 32;
pub struct NeighborCache {
    entries: [Option<NeighborEntry>; MAX_IPV6_NEIGHBORS],
}

impl NeighborCache {
    pub const MAX_TTL: u64 = 86_400; // 24 hours max upper bound

    #[inline]
    pub fn is_valid_unicast_mac(mac: [u8; 6]) -> bool {
        (mac[0] & 0x01 == 0) && mac != [0u8; 6]
    }

    #[inline]
    pub fn is_valid_neighbor_ip(addr: Ipv6Address) -> bool {
        !addr.is_multicast() && !addr.is_unspecified() && !addr.is_loopback()
    }

    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_IPV6_NEIGHBORS],
        }
    }

    pub fn learn_advertisement(
        &mut self,
        address: Ipv6Address,
        mac: [u8; 6],
        override_flag: bool,
        now: u64,
        ttl: u64,
    ) -> Result<(), &'static str> {
        self.learn_advertisement_with_router(address, mac, false, override_flag, now, ttl)
    }

    pub fn learn_advertisement_with_router(
        &mut self,
        address: Ipv6Address,
        mac: [u8; 6],
        is_router: bool,
        override_flag: bool,
        now: u64,
        ttl: u64,
    ) -> Result<(), &'static str> {
        if !Self::is_valid_neighbor_ip(address) {
            return Err("invalid neighbor IP (multicast/unspecified)");
        }
        if !Self::is_valid_unicast_mac(mac) {
            return Err("invalid neighbor MAC (multicast/broadcast/zero)");
        }
        if ttl == 0 {
            return Err("TTL must be greater than zero");
        }
        let bounded_ttl = ttl.min(Self::MAX_TTL);
        let reachable_until = now.saturating_add(bounded_ttl);

        if let Some(entry) = self.entries.iter_mut().flatten().find(|e| e.address == address) {
            if entry.mac != mac && !override_flag {
                // RFC 4861 § 7.2.5: Do NOT update link-layer address without override flag!
                if !is_router {
                    entry.is_router = false;
                }
                return Ok(());
            }
            entry.mac = mac;
            entry.is_router = is_router;
            entry.reachable_until = reachable_until;
            return Ok(());
        }

        self.expire(now);
        if let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some(NeighborEntry {
                address,
                mac,
                is_router,
                reachable_until,
            });
            return Ok(());
        }

        // Cache saturated with active entries: evict entry closest to expiry
        let mut min_idx = 0;
        let mut min_expiry = u64::MAX;
        for (i, entry) in self.entries.iter().enumerate() {
            if let Some(e) = entry {
                if e.reachable_until < min_expiry {
                    min_expiry = e.reachable_until;
                    min_idx = i;
                }
            }
        }
        self.entries[min_idx] = Some(NeighborEntry {
            address,
            mac,
            is_router,
            reachable_until,
        });
        Ok(())
    }

    pub fn learn(&mut self, address: Ipv6Address, mac: [u8; 6], now: u64, ttl: u64) {
        let _ = self.learn_advertisement_with_router(address, mac, false, false, now, ttl);
    }

    pub fn lookup(&self, address: Ipv6Address, now: u64) -> Option<[u8; 6]> {
        self.entries
            .iter()
            .flatten()
            .find(|e| e.address == address && now <= e.reachable_until)
            .map(|e| e.mac)
    }

    pub fn expire(&mut self, now: u64) {
        for e in &mut self.entries {
            if e.map(|x| now > x.reachable_until).unwrap_or(false) {
                *e = None;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Autoconfig {
    pub prefix: Ipv6Prefix,
    pub interface_id: [u8; 8],
}

impl Ipv6Autoconfig {
    pub fn new(prefix: Ipv6Prefix, mac: [u8; 6]) -> Self {
        let interface_id = [
            mac[0] ^ 0x02,
            mac[1],
            mac[2],
            0xff,
            0xfe,
            mac[3],
            mac[4],
            mac[5],
        ];
        Self {
            prefix,
            interface_id,
        }
    }

    pub fn from_mac_address(prefix: Ipv6Prefix, mac: super::ethernet::MacAddress) -> Self {
        Self::new(prefix, mac.0)
    }

    pub fn valid(&self)->bool {
        self.prefix.prefix_len == 64
            && !self.prefix.address.is_multicast()
            && !self.prefix.address.is_link_local()
    }

    pub fn address(&self)->Option<Ipv6Address> {
        if !self.valid() {
            return None;
        }
        if self.interface_id == [0u8; 8]
            || self.interface_id == [0xfd, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80]
        {
            return None;
        }
        let mut a = self.prefix.address.0;
        a[8..16].copy_from_slice(&self.interface_id);
        Some(Ipv6Address(a))
    }
}

pub const ICMPV6_NEIGHBOR_SOLICITATION: u8 = 135;
pub const ICMPV6_NEIGHBOR_ADVERTISEMENT: u8 = 136;
pub const ICMPV6_ROUTER_SOLICITATION: u8 = 133;
pub const ICMPV6_ROUTER_ADVERTISEMENT: u8 = 134;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icmpv6Neighbor {
    pub kind: u8,
    pub target: Ipv6Address,
    pub router: bool,
    pub solicited: bool,
    pub override_flag: bool,
    pub link_layer_address: Option<[u8; 6]>,
}

impl Icmpv6Neighbor {
    pub fn parse(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < 24 {
            return Err("ICMPv6 neighbor message too short");
        }
        let kind = buf[0];
        if kind != ICMPV6_NEIGHBOR_SOLICITATION && kind != ICMPV6_NEIGHBOR_ADVERTISEMENT {
            return Err("unsupported ICMPv6 neighbor message");
        }
        if buf[1] != 0 {
            return Err("invalid ICMPv6 neighbor code");
        }
        let target = Ipv6Address(
            buf[8..24]
                .try_into()
                .map_err(|_| "invalid ICMPv6 target")?,
        );
        // RFC 4861 § 4.3, 4.4, 7.1.1 & 7.1.2: Target Address MUST NOT be multicast or unspecified
        if target.is_multicast() || target.is_unspecified() {
            return Err("ICMPv6 target address must not be multicast or unspecified");
        }
        let flags = if kind == ICMPV6_NEIGHBOR_ADVERTISEMENT {
            buf[4]
        } else {
            0
        };

        // Parse ICMPv6 options starting at offset 24 (SLLA/TLLA options)
        let mut link_layer_address = None;
        let mut p = 24usize;
        while p + 2 <= buf.len() {
            let opt_type = buf[p];
            let opt_len = (buf[p + 1] as usize) * 8;
            if opt_len < 8 || p + opt_len > buf.len() {
                return Err("malformed ICMPv6 neighbor option");
            }
            // RFC 4861 § 4.3 & 4.4:
            // Option 1 (SLLA) is ONLY valid on Neighbor Solicitation.
            // Option 2 (TLLA) is ONLY valid on Neighbor Advertisement.
            // Options 3 (Prefix Info) and 4 (Redirected Header) are illegal in NS and NA.
            if kind == ICMPV6_NEIGHBOR_SOLICITATION && opt_type == 2 {
                return Err("TLLA option illegal in Neighbor Solicitation");
            }
            if kind == ICMPV6_NEIGHBOR_SOLICITATION && (opt_type == 3 || opt_type == 4) {
                return Err("illegal option in Neighbor Solicitation");
            }
            if kind == ICMPV6_NEIGHBOR_ADVERTISEMENT && opt_type == 1 {
                return Err("SLLA option illegal in Neighbor Advertisement");
            }
            if kind == ICMPV6_NEIGHBOR_ADVERTISEMENT && (opt_type == 3 || opt_type == 4) {
                return Err("illegal option in Neighbor Advertisement");
            }
            // For Ethernet, SLLA/TLLA option length MUST be exactly 8 bytes (1 unit of 8 octets)
            if opt_type == 1 || opt_type == 2 {
                if opt_len != 8 {
                    return Err("illegal link-layer address option length for Ethernet");
                }
                if link_layer_address.is_some() {
                    return Err("duplicate link-layer address option");
                }
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&buf[p + 2..p + 8]);
                if !NeighborCache::is_valid_unicast_mac(mac) {
                    return Err("illegal multicast or zero MAC in ICMPv6 option");
                }
                link_layer_address = Some(mac);
            }
            p += opt_len;
        }

        if p != buf.len() {
            return Err("malformed ICMPv6 neighbor option trailing bytes");
        }

        Ok(Self {
            kind,
            target,
            router: flags & 0x80 != 0,
            solicited: flags & 0x40 != 0,
            override_flag: flags & 0x20 != 0,
            link_layer_address,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icmpv6PrefixInfo {
    pub prefix: Ipv6Prefix,
    pub on_link: bool,
    pub autonomous: bool,
    pub valid_lifetime: u32,
    pub preferred_lifetime: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icmpv6RouterAdvertisement {
    pub hop_limit: u8,
    pub router_lifetime: u16,
    pub reachable_time: u32,
    pub retrans_timer: u32,
    pub prefix: Option<Ipv6Prefix>,
    pub prefix_info: Option<Icmpv6PrefixInfo>,
}

impl Icmpv6RouterAdvertisement {
    pub fn parse(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < 16 || buf[0] != ICMPV6_ROUTER_ADVERTISEMENT || buf[1] != 0 {
            return Err("invalid ICMPv6 router advertisement");
        }
        let mut prefix = None;
        let mut prefix_info = None;
        let mut p = 16usize;
        while p + 2 <= buf.len() {
            let typ = buf[p];
            let len = (buf[p + 1] as usize) * 8;
            if len < 8 || p + len > buf.len() {
                return Err("invalid ICMPv6 RA option");
            }
            // Option 3 = Prefix Information Option (RFC 4861 § 4.6.2)
            if typ == 3 && len >= 32 {
                let plen = buf[p + 2];
                let flags = buf[p + 3];
                let on_link = (flags & 0x80) != 0;
                let autonomous = (flags & 0x40) != 0;
                let valid_lifetime = u32::from_be_bytes([buf[p + 4], buf[p + 5], buf[p + 6], buf[p + 7]]);
                let preferred_lifetime = u32::from_be_bytes([buf[p + 8], buf[p + 9], buf[p + 10], buf[p + 11]]);

                if plen <= 128 && preferred_lifetime <= valid_lifetime {
                    let mut a = [0u8; 16];
                    a.copy_from_slice(&buf[p + 16..p + 32]);
                    let addr = Ipv6Address(a);
                    if !addr.is_multicast() && !addr.is_link_local() {
                        let pref = Ipv6Prefix::new(addr, plen);
                        prefix_info = Some(Icmpv6PrefixInfo {
                            prefix: pref,
                            on_link,
                            autonomous,
                            valid_lifetime,
                            preferred_lifetime,
                        });
                        if autonomous && valid_lifetime > 0 {
                            prefix = Some(pref);
                        }
                    }
                }
            }
            p += len;
        }
        Ok(Self {
            hop_limit: buf[4],
            router_lifetime: u16::from_be_bytes([buf[6], buf[7]]),
            reachable_time: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
            retrans_timer: u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]),
            prefix,
            prefix_info,
        })
    }
}

/// IPv6 address lifecycle state machine per RFC 4862.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Ipv6AddressState {
    Tentative,
    Preferred,
    Deprecated,
    Invalid,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Ipv6AddressEntry {
    pub address: Ipv6Address,
    pub state: Ipv6AddressState,
    pub valid_until: u64,
    pub preferred_until: u64,
    pub dad_transmits_remaining: u8,
}

impl Ipv6AddressEntry {
    pub const fn empty() -> Self {
        Self {
            address: Ipv6Address::UNSPECIFIED,
            state: Ipv6AddressState::Invalid,
            valid_until: 0,
            preferred_until: 0,
            dad_transmits_remaining: 0,
        }
    }
}

pub const MAX_IPV6_ADDRS: usize = 4;
pub struct Ipv6AddressTable {
    pub addrs: [Ipv6AddressEntry; MAX_IPV6_ADDRS],
}

impl Ipv6AddressTable {
    pub const fn new() -> Self {
        Self {
            addrs: [Ipv6AddressEntry::empty(); MAX_IPV6_ADDRS],
        }
    }

    pub fn assign_tentative(&mut self, addr: Ipv6Address, valid_until: u64, preferred_until: u64) -> Result<(), &'static str> {
        if addr.is_unspecified() || addr.is_multicast() {
            return Err("illegal IPv6 address");
        }
        for entry in self.addrs.iter_mut() {
            if entry.state != Ipv6AddressState::Invalid && entry.address == addr {
                return Ok(());
            }
        }
        for entry in self.addrs.iter_mut() {
            if entry.state == Ipv6AddressState::Invalid {
                *entry = Ipv6AddressEntry {
                    address: addr,
                    state: Ipv6AddressState::Tentative,
                    valid_until,
                    preferred_until,
                    dad_transmits_remaining: 1,
                };
                return Ok(());
            }
        }
        Err("IPv6 address table full")
    }

    pub fn mark_preferred(&mut self, addr: Ipv6Address) -> bool {
        for entry in self.addrs.iter_mut() {
            if entry.address == addr && entry.state == Ipv6AddressState::Tentative {
                entry.state = Ipv6AddressState::Preferred;
                return true;
            }
        }
        false
    }

    pub fn mark_invalid(&mut self, addr: Ipv6Address) -> bool {
        for entry in self.addrs.iter_mut() {
            if entry.address == addr {
                entry.state = Ipv6AddressState::Invalid;
                entry.valid_until = 0;
                entry.preferred_until = 0;
                return true;
            }
        }
        false
    }

    pub fn withdraw_prefix(&mut self, prefix: Ipv6Prefix) {
        for entry in self.addrs.iter_mut() {
            if entry.state != Ipv6AddressState::Invalid && prefix.contains(entry.address) {
                entry.state = Ipv6AddressState::Invalid;
                entry.valid_until = 0;
                entry.preferred_until = 0;
            }
        }
    }

    pub fn contains_tentative(&self, addr: Ipv6Address) -> bool {
        self.addrs.iter().any(|e| e.state == Ipv6AddressState::Tentative && e.address == addr)
    }

    pub fn contains_assigned(&self, addr: Ipv6Address) -> bool {
        self.addrs.iter().any(|e| (e.state == Ipv6AddressState::Preferred || e.state == Ipv6AddressState::Deprecated) && e.address == addr)
    }
}

/// Builds an RFC 4862 Duplicate Address Detection (DAD) probe frame.
/// Source IPv6 is :: (unspecified), Destination is solicited-node multicast (ff02::1:ffxx:xxxx).
/// Destination MAC is 33:33:ff:xx:xx:xx.
pub fn build_dad_probe_frame(
    tentative_addr: Ipv6Address,
    local_mac: crate::net::ethernet::MacAddress,
    out: &mut [u8; 1514],
) -> Result<usize, &'static str> {
    let sol_node = tentative_addr.solicited_node_multicast();
    let mcast_mac = sol_node.multicast_mac();

    let mut ns_buf = [0u8; 24]; // 24 bytes NS without SLLA (RFC 4861 § 7.1.1 forbids SLLA on DAD probe)
    ns_buf[0] = 135; // NS
    ns_buf[1] = 0;
    ns_buf[8..24].copy_from_slice(&tentative_addr.0);

    let csum = calculate_icmpv6_checksum(&Ipv6Address::UNSPECIFIED, &sol_node, &ns_buf);
    ns_buf[2..4].copy_from_slice(&csum.to_be_bytes());

    let ip6 = Ipv6Packet {
        src: Ipv6Address::UNSPECIFIED,
        dst: sol_node,
        next_header: 58,
        hop_limit: 255,
        payload: &ns_buf,
    };
    let mut ip6_buf = [0u8; 128];
    let ip6_len = ip6.serialize(&mut ip6_buf)?;

    let eth = crate::net::ethernet::EthernetFrame {
        dst_mac: mcast_mac,
        src_mac: local_mac,
        ethertype: crate::net::ethernet::EtherType::Ipv6,
        payload: &ip6_buf[..ip6_len],
    };
    eth.serialize(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = Ipv6Packet {
            src: Ipv6Address::LOCALHOST,
            dst: Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            next_header: 17,
            hop_limit: 64,
            payload: b"interos",
        };
        let mut b = [0u8; 64];
        let n = p.serialize(&mut b).unwrap();
        let q = Ipv6Packet::parse(&b[..n]).unwrap();
        assert_eq!(q.src, p.src);
        assert_eq!(q.dst, p.dst);
        assert_eq!(q.payload, p.payload);
    }

    #[test]
    fn neighbor_cache_learn_expires_and_recycles_stale_slots() {
        let mut cache = NeighborCache::new();
        for i in 0..MAX_IPV6_NEIGHBORS {
            let mut addr = [0u8; 16];
            addr[0] = 0x20;
            addr[15] = (i + 1) as u8;
            cache.learn(
                Ipv6Address(addr),
                [0x52, 0x54, 0x00, 0x12, 0x34, (i + 1) as u8],
                100,
                10,
            );
        }
        assert_eq!(cache.len(), MAX_IPV6_NEIGHBORS);

        let test_addr = Ipv6Address([0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]);
        assert!(cache.lookup(test_addr, 105).is_some());

        let new_addr = Ipv6Address([0x20, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x99]);
        cache.learn(new_addr, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff], 115, 100);

        assert_eq!(
            cache.lookup(new_addr, 115),
            Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])
        );
        assert_eq!(cache.lookup(test_addr, 115), None, "expired entry must not be returned");
        assert_eq!(cache.len(), 1, "only the newly learned entry should be in cache");
    }

    #[test]
    fn ipv6_routing_table_longest_prefix_match() {
        let mut table = Ipv6RoutingTable::new();
        let default_route = Ipv6Route {
            prefix: Ipv6Prefix {
                address: Ipv6Address([0; 16]),
                prefix_len: 0,
            },
            next_hop: Some(Ipv6Address([
                0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            ])),
            interface: 1,
            metric: 100,
        };
        let specific_route = Ipv6Route {
            prefix: Ipv6Prefix {
                address: Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                prefix_len: 32,
            },
            next_hop: Some(Ipv6Address([
                0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2,
            ])),
            interface: 2,
            metric: 10,
        };
        table.add(default_route).expect("add default");
        table.add(specific_route).expect("add specific");

        let target_match = Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x55]);
        let matched = table.lookup(target_match).expect("must find route");
        assert_eq!(matched.interface, 2, "must choose /32 specific route over default /0");

        let other_target = Ipv6Address([0x26, 0, 0x14, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let matched_default = table.lookup(other_target).expect("must find default route");
        assert_eq!(matched_default.interface, 1, "must fallback to default route");
    }

    #[test]
    fn ndp_hop_limit_enforcement() {
        assert!(!verify_ndp_hop_limit(254));
        assert!(!verify_ndp_hop_limit(64));
        assert!(!verify_ndp_hop_limit(1));
        assert!(verify_ndp_hop_limit(255));
    }

    #[test]
    fn neighbor_cache_rejects_illegal_mac_and_multicast_ip() {
        let mut cache = NeighborCache::new();
        let valid_ip = Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let mcast_ip = Ipv6Address([0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let bcast_mac = [0xff; 6];
        let mcast_mac = [0x33, 0x33, 0x00, 0x00, 0x00, 0x01];
        let zero_mac = [0x00; 6];
        let valid_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

        assert!(cache.learn_advertisement(valid_ip, bcast_mac, true, 100, 60).is_err());
        assert!(cache.learn_advertisement(valid_ip, mcast_mac, true, 100, 60).is_err());
        assert!(cache.learn_advertisement(valid_ip, zero_mac, true, 100, 60).is_err());
        assert!(cache.learn_advertisement(mcast_ip, valid_mac, true, 100, 60).is_err());
    }

    #[test]
    fn neighbor_cache_respects_override_flag() {
        let mut cache = NeighborCache::new();
        let ip = Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let mac1 = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mac2 = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];

        cache.learn_advertisement(ip, mac1, true, 100, 60).unwrap();
        assert_eq!(cache.lookup(ip, 105), Some(mac1));

        // Unsolicited NA without Override flag must NOT overwrite existing MAC
        cache.learn_advertisement(ip, mac2, false, 110, 60).unwrap();
        assert_eq!(cache.lookup(ip, 115), Some(mac1), "MAC must NOT be updated when override=false");

        // NA with Override flag MUST overwrite existing MAC
        cache.learn_advertisement(ip, mac2, true, 120, 60).unwrap();
        assert_eq!(cache.lookup(ip, 125), Some(mac2), "MAC must be updated when override=true");
    }

    #[test]
    fn routing_table_deduplication_and_removal() {
        let mut table = Ipv6RoutingTable::new();
        let prefix = Ipv6Prefix {
            address: Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            prefix_len: 32,
        };
        let r1 = Ipv6Route { prefix, next_hop: None, interface: 1, metric: 100 };
        let r2 = Ipv6Route { prefix, next_hop: None, interface: 2, metric: 50 };

        table.add(r1).unwrap();
        assert_eq!(table.len(), 1);
        table.add(r2).unwrap();
        assert_eq!(table.len(), 1, "Duplicate prefix must update in place rather than creating new entry");
        assert_eq!(
            table.lookup(Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])).unwrap().interface,
            2
        );

        assert!(table.remove_prefix(prefix));
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn slaac_rejects_multicast_and_link_local_prefixes() {
        let mcast_prefix = Ipv6Prefix {
            address: Ipv6Address([0xff, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            prefix_len: 64,
        };
        let ll_prefix = Ipv6Prefix {
            address: Ipv6Address([0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            prefix_len: 64,
        };
        let global_prefix = Ipv6Prefix {
            address: Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            prefix_len: 64,
        };
        let iid = [0x50, 0x54, 0x00, 0xff, 0xfe, 0x12, 0x34, 0x56];

        assert!(!Ipv6Autoconfig { prefix: mcast_prefix, interface_id: iid }.valid());
        assert!(!Ipv6Autoconfig { prefix: ll_prefix, interface_id: iid }.valid());
        assert!(Ipv6Autoconfig { prefix: global_prefix, interface_id: iid }.valid());
    }

    #[test]
    fn icmpv6_checksum_roundtrip_and_verification() {
        let src = Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let dst = Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        let mut icmp_msg = [
            128u8, 0, // Echo Request (Type 128, Code 0)
            0, 0,     // Checksum placeholder
            0x12, 0x34, // Identifier
            0x00, 0x01, // Sequence Number
            b'P', b'I', b'N', b'G',
        ];
        let csum = calculate_icmpv6_checksum(&src, &dst, &icmp_msg);
        icmp_msg[2..4].copy_from_slice(&csum.to_be_bytes());

        assert!(verify_icmpv6_checksum(&src, &dst, &icmp_msg));
        // Tampered payload must fail checksum
        icmp_msg[8] = b'X';
        assert!(!verify_icmpv6_checksum(&src, &dst, &icmp_msg));
    }

    #[test]
    fn icmpv6_neighbor_parses_target_link_layer_and_rejects_multicast() {
        let mut valid_na = [
            136u8, 0, 0, 0, // Type 136 (NA), Code 0, Checksum
            0x60, 0, 0, 0, // Flags: R=0, S=1, O=1
            // Target address: 2001:db8::1
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            // Option 2 (TLLA), Length 1 (8 bytes)
            2, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56,
        ];
        let parsed = Icmpv6Neighbor::parse(&valid_na).expect("valid NA");
        assert_eq!(parsed.kind, 136);
        assert!(parsed.solicited);
        assert!(parsed.override_flag);
        assert_eq!(parsed.link_layer_address, Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));

        // Target with multicast prefix ff02::1 MUST be rejected
        valid_na[8] = 0xff;
        valid_na[9] = 0x02;
        assert!(Icmpv6Neighbor::parse(&valid_na).is_err());

        // Valid Neighbor Solicitation (Type 135) with SLLA (Option 1)
        let valid_ns = [
            135u8, 0, 0, 0, // Type 135 (NS), Code 0, Checksum
            0, 0, 0, 0,     // Reserved
            // Target address: 2001:db8::1
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            // Option 1 (SLLA), Length 1 (8 bytes)
            1, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56,
        ];
        let parsed_ns = Icmpv6Neighbor::parse(&valid_ns).expect("valid NS");
        assert_eq!(parsed_ns.kind, 135);
        assert_eq!(parsed_ns.link_layer_address, Some([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]));

        // RFC 4861 § 4.3 violation: TLLA (Option 2) inside NS MUST be rejected
        let mut illegal_ns = valid_ns;
        illegal_ns[24] = 2; // Option 2 (TLLA) in NS
        assert_eq!(
            Icmpv6Neighbor::parse(&illegal_ns),
            Err("TLLA option illegal in Neighbor Solicitation")
        );

        // RFC 4861 § 4.4 violation: SLLA (Option 1) inside NA MUST be rejected
        let illegal_na = [
            136u8, 0, 0, 0,
            0x60, 0, 0, 0,
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            1, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56, // Option 1 in NA
        ];
        assert_eq!(
            Icmpv6Neighbor::parse(&illegal_na),
            Err("SLLA option illegal in Neighbor Advertisement")
        );

        // Truncated option: Length specifies 1 unit (8 bytes), but only 6 provided
        let truncated_opt = [
            135u8, 0, 0, 0,
            0, 0, 0, 0,
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            1, 1, 0x52, 0x54, 0x00, 0x12, // Only 6 bytes!
        ];
        assert_eq!(
            Icmpv6Neighbor::parse(&truncated_opt),
            Err("malformed ICMPv6 neighbor option")
        );

        // Multicast MAC in SLLA option MUST be rejected
        let mut mcast_mac_ns = valid_ns;
        mcast_mac_ns[26] = 0x01; // Multicast bit set in MAC
        assert_eq!(
            Icmpv6Neighbor::parse(&mcast_mac_ns),
            Err("illegal multicast or zero MAC in ICMPv6 option")
        );
    }

    #[test]
    fn ndp_option_parser_rejects_duplicate_options_and_trailing_bytes() {
        // Valid NA with 1 TLLA
        let valid_na = [
            136u8, 0, 0, 0,
            0x60, 0, 0, 0,
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            2, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56, // TLLA 1
        ];
        assert!(Icmpv6Neighbor::parse(&valid_na).is_ok());

        // Duplicate TLLA option in NA MUST be rejected
        let duplicate_tlla_na = [
            136u8, 0, 0, 0,
            0x60, 0, 0, 0,
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            2, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56, // TLLA 1
            2, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x77, // TLLA 2 (duplicate!)
        ];
        assert_eq!(
            Icmpv6Neighbor::parse(&duplicate_tlla_na),
            Err("duplicate link-layer address option")
        );

        // Trailing non-option junk byte MUST be rejected
        let trailing_byte_na = [
            136u8, 0, 0, 0,
            0x60, 0, 0, 0,
            0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            2, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56,
            0xFF, // Trailing junk byte
        ];
        assert_eq!(
            Icmpv6Neighbor::parse(&trailing_byte_na),
            Err("malformed ICMPv6 neighbor option trailing bytes")
        );
    }

    #[test]
    fn ndp_target_address_rejects_unspecified() {
        let unspecified_target_ns = [
            135u8, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // :: (unspecified target)
            1, 1, 0x52, 0x54, 0x00, 0x12, 0x34, 0x56,
        ];
        assert_eq!(
            Icmpv6Neighbor::parse(&unspecified_target_ns),
            Err("ICMPv6 target address must not be multicast or unspecified")
        );
    }

    #[test]
    fn neighbor_cache_ns_slla_cannot_poison_existing_entry() {
        let mut cache = NeighborCache::new();
        let target = Ipv6Address([0x20, 1, 0xdb, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        let legit_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let attacker_mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];

        // Establish legitimate entry (e.g. from authenticated router/NA)
        cache.learn_advertisement(target, legit_mac, true, 100, 300).unwrap();
        assert_eq!(cache.lookup(target, 105), Some(legit_mac));

        // Attacker sends Neighbor Solicitation with SLLA (override_flag = false)
        cache.learn_advertisement_with_router(target, attacker_mac, false, false, 110, 300).unwrap();
        assert_eq!(
            cache.lookup(target, 115),
            Some(legit_mac),
            "NS SLLA MUST NOT poison existing neighbor cache entry!"
        );
    }
}
