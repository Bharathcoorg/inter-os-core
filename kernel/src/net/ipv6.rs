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
