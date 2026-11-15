// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bounded IPv6 packet primitives for the InterOS network stack.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Address(pub [u8; 16]);

impl Ipv6Address {
    pub const LOCALHOST: Self = Self([0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Ipv6NextHeader { HopByHop=0, Tcp=6, Udp=17, Icmpv6=58, NoNext=59 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Packet<'a> {
    pub src: Ipv6Address,
    pub dst: Ipv6Address,
    pub next_header: u8,
    pub hop_limit: u8,
    pub payload: &'a [u8],
}

impl<'a> Ipv6Packet<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 40 { return Err("IPv6 header too short"); }
        if data[0] >> 4 != 6 { return Err("invalid IPv6 version"); }
        let payload_len = u16::from_be_bytes([data[4], data[5]]) as usize;
        if data.len() != 40 + payload_len { return Err("IPv6 payload length mismatch"); }
        let mut src=[0u8;16]; src.copy_from_slice(&data[8..24]);
        let mut dst=[0u8;16]; dst.copy_from_slice(&data[24..40]);
        Ok(Self { src:Ipv6Address(src), dst:Ipv6Address(dst), next_header:data[6], hop_limit:data[7], payload:&data[40..] })
    }

    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if self.payload.len() > u16::MAX as usize || out.len() < 40 + self.payload.len() {
            return Err("IPv6 packet too large");
        }
        out[..40+self.payload.len()].fill(0);
        out[0]=0x60;
        let len=(self.payload.len() as u16).to_be_bytes();
        out[4..6].copy_from_slice(&len);
        out[6]=self.next_header;
        out[7]=self.hop_limit;
        out[8..24].copy_from_slice(&self.src.0);
        out[24..40].copy_from_slice(&self.dst.0);
        out[40..40+self.payload.len()].copy_from_slice(self.payload);
        Ok(40+self.payload.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let p=Ipv6Packet{src:Ipv6Address::LOCALHOST,dst:Ipv6Address([0x20,1,0xdb,8,0,0,0,0,0,0,0,0,0,0,0,1]),next_header:17,hop_limit:64,payload:b"interos"};
        let mut b=[0u8;64];
        let n=p.serialize(&mut b).unwrap();
        let q=Ipv6Packet::parse(&b[..n]).unwrap();
        assert_eq!(q.src,p.src); assert_eq!(q.dst,p.dst); assert_eq!(q.payload,p.payload);
    }

    #[test]
    fn neighbor_cache_learn_expires_and_recycles_stale_slots() {
        let mut cache = NeighborCache::new();
        // Fill all 32 entries with TTL = 10 at now = 100 (expire at 110)
        for i in 0..MAX_IPV6_NEIGHBORS {
            let mut addr = [0u8; 16];
            addr[15] = i as u8;
            cache.learn(Ipv6Address(addr), [0x52, 0x54, 0x00, 0x12, 0x34, i as u8], 100, 10);
        }
        assert_eq!(cache.len(), MAX_IPV6_NEIGHBORS);

        // At now = 105, lookup should succeed
        let test_addr = Ipv6Address([0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,5]);
        assert!(cache.lookup(test_addr, 105).is_some());

        // At now = 115, all previous entries are expired.
        // Calling learn on a new 33rd entry must expire the stale entries and succeed in inserting!
        let new_addr = Ipv6Address([0x20, 0x01, 0,0,0,0,0,0,0,0,0,0,0,0,0,0x99]);
        cache.learn(new_addr, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff], 115, 100);

        assert_eq!(cache.lookup(new_addr, 115), Some([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]));
        assert_eq!(cache.lookup(test_addr, 115), None, "expired entry must not be returned");
        assert_eq!(cache.len(), 1, "only the newly learned entry should be in cache");
    }

    #[test]
    fn ipv6_routing_table_longest_prefix_match() {
        let mut table = Ipv6RoutingTable::new();
        let default_route = Ipv6Route {
            prefix: Ipv6Prefix { address: Ipv6Address([0; 16]), prefix_len: 0 },
            next_hop: Some(Ipv6Address([0xfe, 0x80, 0,0,0,0,0,0,0,0,0,0,0,0,0,1])),
            interface: 1,
            metric: 100,
        };
        let specific_route = Ipv6Route {
            prefix: Ipv6Prefix { address: Ipv6Address([0x20, 1, 0xdb, 8, 0,0,0,0,0,0,0,0,0,0,0,0]), prefix_len: 32 },
            next_hop: Some(Ipv6Address([0xfe, 0x80, 0,0,0,0,0,0,0,0,0,0,0,0,0,2])),
            interface: 2,
            metric: 10,
        };
        table.add(default_route).expect("add default");
        table.add(specific_route).expect("add specific");

        let target_match = Ipv6Address([0x20, 1, 0xdb, 8, 0,0,0,0,0,0,0,0,0,0,0,0x55]);
        let matched = table.lookup(target_match).expect("must find route");
        assert_eq!(matched.interface, 2, "must choose /32 specific route over default /0");

        let other_target = Ipv6Address([0x26, 0, 0x14, 0x06, 0,0,0,0,0,0,0,0,0,0,0,1]);
        let matched_default = table.lookup(other_target).expect("must find default route");
        assert_eq!(matched_default.interface, 1, "must fallback to default route");
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Prefix { pub address: Ipv6Address, pub prefix_len: u8 }
impl Ipv6Prefix { pub fn contains(&self, ip:Ipv6Address)->bool { if self.prefix_len>128{return false} let full=(self.prefix_len/8) as usize; let rem=self.prefix_len%8; if self.address.0[..full]!=ip.0[..full]{return false} if rem==0{return true} let mask=0xffu8<<(8-rem); (self.address.0[full]&mask)==(ip.0[full]&mask) } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Route { pub prefix:Ipv6Prefix, pub next_hop:Option<Ipv6Address>, pub interface:u8, pub metric:u16 }

pub const MAX_IPV6_ROUTES:usize=32;
pub struct Ipv6RoutingTable { routes:[Option<Ipv6Route>;MAX_IPV6_ROUTES] }
impl Ipv6RoutingTable {
 pub const fn new()->Self{Self{routes:[None;MAX_IPV6_ROUTES]}}
 pub fn add(&mut self,r:Ipv6Route)->Result<(),Ipv6Route>{if r.prefix.prefix_len>128{return Err(r)};if let Some(s)=self.routes.iter_mut().find(|x|x.is_none()){*s=Some(r);Ok(())}else{Err(r)}}
 pub fn lookup(&self,dst:Ipv6Address)->Option<Ipv6Route>{self.routes.iter().flatten().filter(|r|r.prefix.contains(dst)).max_by_key(|r|(r.prefix.prefix_len as u16,u16::MAX-r.metric)).copied()}
 pub fn len(&self)->usize{self.routes.iter().filter(|r|r.is_some()).count()}
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeighborEntry { pub address: Ipv6Address, pub mac: [u8;6], pub reachable_until: u64 }

pub const MAX_IPV6_NEIGHBORS: usize = 32;
pub struct NeighborCache { entries:[Option<NeighborEntry>;MAX_IPV6_NEIGHBORS] }
impl NeighborCache {
 pub const fn new()->Self{Self{entries:[None;MAX_IPV6_NEIGHBORS]}}
 pub fn learn(&mut self,address:Ipv6Address,mac:[u8;6],now:u64,ttl:u64){
  if let Some(e)=self.entries.iter_mut().flatten().find(|e|e.address==address){*e=NeighborEntry{address,mac,reachable_until:now.saturating_add(ttl)};return;}
  self.expire(now);
  if let Some(slot)=self.entries.iter_mut().find(|e|e.is_none()){*slot=Some(NeighborEntry{address,mac,reachable_until:now.saturating_add(ttl)});}
 }
 pub fn lookup(&self,address:Ipv6Address,now:u64)->Option<[u8;6]>{self.entries.iter().flatten().find(|e|e.address==address&&now<=e.reachable_until).map(|e|e.mac)}
 pub fn expire(&mut self,now:u64){for e in &mut self.entries{if e.map(|x|now>x.reachable_until).unwrap_or(false){*e=None;}}}
 pub fn len(&self)->usize{self.entries.iter().filter(|e|e.is_some()).count()}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv6Autoconfig { pub prefix:Ipv6Prefix, pub interface_id:[u8;8] }
impl Ipv6Autoconfig {
 pub fn valid(&self)->bool { self.prefix.prefix_len == 64 }
 pub fn address(&self)->Option<Ipv6Address> {
  if !self.valid() { return None; }
  let mut a=self.prefix.address.0;
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
}

impl Icmpv6Neighbor {
    pub fn parse(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < 24 { return Err("ICMPv6 neighbor message too short"); }
        let kind=buf[0];
        if kind != ICMPV6_NEIGHBOR_SOLICITATION && kind != ICMPV6_NEIGHBOR_ADVERTISEMENT {
            return Err("unsupported ICMPv6 neighbor message");
        }
        if buf[1] != 0 { return Err("invalid ICMPv6 neighbor code"); }
        let target=Ipv6Address(buf[8..24].try_into().map_err(|_| "invalid ICMPv6 target")?);
        let flags=if kind==ICMPV6_NEIGHBOR_ADVERTISEMENT { buf[4] } else { 0 };
        Ok(Self{kind,target,router:flags&0x80!=0,solicited:flags&0x40!=0,override_flag:flags&0x20!=0})
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Icmpv6RouterAdvertisement {
    pub hop_limit: u8,
    pub router_lifetime: u16,
    pub reachable_time: u32,
    pub retrans_timer: u32,
    pub prefix: Option<Ipv6Prefix>,
}

impl Icmpv6RouterAdvertisement {
    pub fn parse(buf: &[u8]) -> Result<Self, &'static str> {
        if buf.len() < 16 || buf[0] != ICMPV6_ROUTER_ADVERTISEMENT || buf[1] != 0 {
            return Err("invalid ICMPv6 router advertisement");
        }
        let mut prefix=None;
        let mut p=16usize;
        while p+2<=buf.len() {
            let typ=buf[p]; let len=(buf[p+1] as usize)*8;
            if len<8 || p+len>buf.len() { return Err("invalid ICMPv6 RA option"); }
            if typ==3 && len>=32 {
                let plen=buf[p+2];
                if plen>128 { return Err("invalid IPv6 prefix length"); }
                let mut a=[0u8;16]; a.copy_from_slice(&buf[p+16..p+32]);
                prefix=Some(Ipv6Prefix{address:Ipv6Address(a),prefix_len:plen});
            }
            p+=len;
        }
        Ok(Self{
            hop_limit:buf[4],
            router_lifetime:u16::from_be_bytes([buf[6],buf[7]]),
            reachable_time:u32::from_be_bytes([buf[8],buf[9],buf[10],buf[11]]),
            retrans_timer:u32::from_be_bytes([buf[12],buf[13],buf[14],buf[15]]),
            prefix,
        })
    }
}
