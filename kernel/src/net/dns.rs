//! Bounded DNS wire-format codec used by the userspace network service.

use crate::net::ipv4::Ipv4Address;

pub const DNS_HEADER_LEN: usize = 12;
pub const DNS_MAX_NAME: usize = 253;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DnsQuestion<'a> {
    pub name: &'a [u8],
    pub qtype: u16,
    pub qclass: u16,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DnsAnswer {
    pub qtype: u16,
    pub ttl: u32,
    pub addr: Ipv4Address,
}

pub fn encode_query(id: u16, name: &[u8], out: &mut [u8]) -> Result<usize, &'static str> {
    if name.is_empty() || name.len() > DNS_MAX_NAME || out.len() < DNS_HEADER_LEN + name.len() + 6 {
        return Err("DNS query buffer too small");
    }
    out[..DNS_HEADER_LEN].fill(0);
    out[0..2].copy_from_slice(&id.to_be_bytes());
    out[2..4].copy_from_slice(&0x0100u16.to_be_bytes());
    out[4..6].copy_from_slice(&1u16.to_be_bytes());
    let mut p = DNS_HEADER_LEN;
    for label in name.split(|b| *b == b'.') {
        if label.is_empty() || label.len() > 63 || p + label.len() + 1 >= out.len() { return Err("invalid DNS name"); }
        out[p] = label.len() as u8; p += 1;
        out[p..p + label.len()].copy_from_slice(label); p += label.len();
    }
    out[p] = 0; p += 1;
    out[p..p+2].copy_from_slice(&1u16.to_be_bytes()); p += 2;
    out[p..p+2].copy_from_slice(&1u16.to_be_bytes()); p += 2;
    Ok(p + 0)
}

pub fn parse_a_answer(buf: &[u8], expected_id: u16) -> Result<DnsAnswer, &'static str> {
    if buf.len() < DNS_HEADER_LEN || u16::from_be_bytes([buf[0], buf[1]]) != expected_id {
        return Err("DNS response id mismatch");
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    let answers = u16::from_be_bytes([buf[6], buf[7]]);
    if flags & 0x8000 == 0 || flags & 0x000F != 0 || answers == 0 { return Err("DNS response rejected"); }
    let mut p = DNS_HEADER_LEN;
    let qd = u16::from_be_bytes([buf[4], buf[5]]);
    for _ in 0..qd {
        loop {
            if p >= buf.len() { return Err("truncated DNS question"); }
            let len = buf[p] as usize; p += 1;
            if len == 0 { break; }
            if len & 0xC0 != 0 || p + len > buf.len() { return Err("invalid DNS label"); }
            p += len;
        }
        if p + 4 > buf.len() { return Err("truncated DNS question"); }
        p += 4;
    }
    for _ in 0..answers {
        if p >= buf.len() { return Err("truncated DNS answer"); }
        if buf[p] & 0xC0 == 0xC0 {
            if p + 2 > buf.len() { return Err("bad DNS name pointer"); }
            p += 2;
        } else {
            loop {
                if p >= buf.len() { return Err("truncated DNS name"); }
                let len = buf[p] as usize; p += 1;
                if len == 0 { break; }
                if len & 0xC0 != 0 || p + len > buf.len() { return Err("invalid DNS name"); }
                p += len;
            }
        }
        if p + 10 > buf.len() { return Err("truncated DNS RR"); }
        let qtype = u16::from_be_bytes([buf[p], buf[p+1]]);
        let class = u16::from_be_bytes([buf[p+2], buf[p+3]]);
        let ttl = u32::from_be_bytes([buf[p+4], buf[p+5], buf[p+6], buf[p+7]]);
        let rdlen = u16::from_be_bytes([buf[p+8], buf[p+9]]) as usize;
        p += 10;
        if p + rdlen > buf.len() { return Err("truncated DNS rdata"); }
        if qtype == 1 && class == 1 && rdlen == 4 {
            return Ok(DnsAnswer { qtype, ttl, addr: Ipv4Address([buf[p], buf[p+1], buf[p+2], buf[p+3]]) });
        }
        p += rdlen;
    }
    Err("no A record")
}


pub const DNS_SERVER: Ipv4Address = Ipv4Address([10, 0, 2, 3]);
pub const DNS_MAX_INFLIGHT: usize = 4;
pub const DNS_MAX_CACHE: usize = 8;
pub const DNS_RETRY_TICKS: u64 = 250;
pub const DNS_MAX_RETRIES: u8 = 3;

#[derive(Copy, Clone)]
pub struct DnsCacheEntry { pub name: [u8; DNS_MAX_NAME], pub name_len: usize, pub addr: Ipv4Address, pub expires_at: u64, pub valid: bool }
impl DnsCacheEntry { pub const fn empty() -> Self { Self { name:[0;DNS_MAX_NAME], name_len:0, addr:Ipv4Address::UNSPECIFIED, expires_at:0, valid:false } } }

#[derive(Copy, Clone)]
pub struct DnsQueryState { pub id:u16, pub name:[u8;DNS_MAX_NAME], pub name_len:usize, pub retries:u8, pub deadline:u64, pub valid:bool }
impl DnsQueryState { pub const fn empty()->Self { Self{id:0,name:[0;DNS_MAX_NAME],name_len:0,retries:0,deadline:0,valid:false} } }

pub struct DnsResolver { next_id:u16, queries:[DnsQueryState;DNS_MAX_INFLIGHT], cache:[DnsCacheEntry;DNS_MAX_CACHE] }
impl DnsResolver {
    pub const fn new()->Self { Self{next_id:1,queries:[DnsQueryState::empty();DNS_MAX_INFLIGHT],cache:[DnsCacheEntry::empty();DNS_MAX_CACHE]} }
    pub fn cached(&self,name:&[u8],now:u64)->Option<Ipv4Address>{ self.cache.iter().find(|e|e.valid&&e.expires_at>now&&e.name_len==name.len()&&e.name[..e.name_len]==*name).map(|e|e.addr) }
    pub fn begin(&mut self,name:&[u8],now:u64)->Result<u16,&'static str>{
        if name.is_empty()||name.len()>DNS_MAX_NAME{return Err("invalid DNS name")}
        if self.cached(name,now).is_some(){return Err("DNS cache hit")}
        let id=self.next_id; self.next_id=self.next_id.wrapping_add(1).max(1);
        let q=self.queries.iter_mut().find(|q|!q.valid).ok_or("DNS query table full")?;
        q.id=id;q.name[..name.len()].copy_from_slice(name);q.name_len=name.len();q.retries=0;q.deadline=now.saturating_add(DNS_RETRY_TICKS);q.valid=true;Ok(id)
    }
    pub fn build_query(&self,id:u16,out:&mut[u8])->Result<usize,&'static str>{let q=self.queries.iter().find(|q|q.valid&&q.id==id).ok_or("unknown DNS query")?;encode_query(id,&q.name[..q.name_len],out)}
    pub fn accept_response(&mut self,packet:&[u8],now:u64)->Result<Ipv4Address,&'static str>{
        let id=u16::from_be_bytes([packet.get(0).ok_or("DNS response too small")?,packet.get(1).ok_or("DNS response too small")?]);
        let a=parse_a_answer(packet,id)?;let q=self.queries.iter_mut().find(|q|q.valid&&q.id==id).ok_or("unknown DNS query")?;
        let mut e=DnsCacheEntry::empty();e.name[..q.name_len].copy_from_slice(&q.name[..q.name_len]);e.name_len=q.name_len;e.addr=a.addr;e.expires_at=now.saturating_add((a.ttl as u64).max(1)*1000);e.valid=true;
        if let Some(dst)=self.cache.iter_mut().find(|e|!e.valid||e.expires_at<=now){*dst=e}else{self.cache[0]=e} q.valid=false;Ok(a.addr)
    }
    pub fn poll_timeouts(&mut self,now:u64)->usize{let mut expired=0;for q in self.queries.iter_mut(){if q.valid&&now>=q.deadline{if q.retries>=DNS_MAX_RETRIES{q.valid=false;expired+=1}else{q.retries+=1;q.deadline=now.saturating_add(DNS_RETRY_TICKS)}}}expired}
    pub fn query(&self,id:u16)->Option<&DnsQueryState>{self.queries.iter().find(|q|q.valid&&q.id==id)}
}

#[cfg(test)]
mod resolver_tests {
 use super::*;
 #[test] fn resolver_query_cache(){let mut r=DnsResolver::new();let id=r.begin(b"example.com",0).unwrap();let mut q=[0u8;300];assert!(r.build_query(id,&mut q).unwrap()>DNS_HEADER_LEN);assert!(r.query(id).is_some());}
 #[test] fn resolver_expires(){let mut r=DnsResolver::new();let id=r.begin(b"example.com",0).unwrap();for n in 1..=4{r.poll_timeouts(DNS_RETRY_TICKS*n as u64);}assert!(r.query(id).is_none());}
}
