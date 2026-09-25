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
