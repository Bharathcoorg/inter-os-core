// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bounded DNS wire-format codec used by the userspace network service.

use crate::net::ipv4::Ipv4Address;

pub const DNS_HEADER_LEN: usize = 12;
pub const DNS_MAX_NAME: usize = 253;
pub const DNS_MAX_POINTER_HOPS: usize = 5;
pub const DNS_MAX_CNAME_HOPS: usize = 4;

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

/// Decodes a DNS wire-format name (with compression pointers) into dotted ASCII in `out`.
///
/// Enforces:
/// - Strictly backward pointers (ptr < current_pos) to prevent pointer loops.
/// - Minimum pointer boundary (ptr >= DNS_HEADER_LEN) to prevent pointing into header.
/// - Maximum recursion depth (hops <= DNS_MAX_POINTER_HOPS) to prevent CPU starvation.
///
/// Returns `(bytes_consumed_in_record, decoded_name_len)`.
pub fn decode_wire_name(
    packet: &[u8],
    start_pos: usize,
    out: &mut [u8; DNS_MAX_NAME],
) -> Result<(usize, usize), &'static str> {
    let mut pos = start_pos;
    let mut out_len = 0usize;
    let mut hops = 0usize;
    let mut bytes_consumed = 0usize;
    let mut encountered_pointer = false;

    loop {
        if pos >= packet.len() {
            return Err("truncated DNS name");
        }
        let len = packet[pos] as usize;
        if (len & 0xC0) == 0xC0 {
            if pos + 2 > packet.len() {
                return Err("bad DNS name pointer");
            }
            let ptr = (((len & 0x3F) as usize) << 8) | (packet[pos + 1] as usize);
            // Strict backward & bounds check to prevent loops & header corruptions
            if ptr >= pos || ptr < DNS_HEADER_LEN {
                return Err("DNS answer name pointer mismatch");
            }
            if hops >= DNS_MAX_POINTER_HOPS {
                return Err("excessive DNS compression pointer hops");
            }
            hops += 1;
            if !encountered_pointer {
                bytes_consumed = (pos + 2) - start_pos;
                encountered_pointer = true;
            }
            pos = ptr;
            continue;
        } else if (len & 0xC0) != 0 {
            return Err("invalid DNS label");
        } else if len == 0 {
            pos += 1;
            if !encountered_pointer {
                bytes_consumed = pos - start_pos;
            }
            break;
        } else {
            pos += 1;
            if pos + len > packet.len() {
                return Err("truncated DNS label");
            }
            if out_len > 0 {
                if out_len >= DNS_MAX_NAME { return Err("DNS name too long"); }
                out[out_len] = b'.';
                out_len += 1;
            }
            if out_len + len > DNS_MAX_NAME { return Err("DNS name too long"); }
            out[out_len..out_len + len].copy_from_slice(&packet[pos..pos + len]);
            out_len += len;
            pos += len;
        }
    }

    Ok((bytes_consumed, out_len))
}

/// Verifies that the question section in a DNS packet matches the expected query name and is A/IN.
pub fn verify_dns_question_matches_query(packet: &[u8], expected_name: &[u8]) -> Result<(), &'static str> {
    if packet.len() < DNS_HEADER_LEN + 5 {
        return Err("truncated DNS header");
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    // QR bit must be 1 (Response): bit 15
    if (flags & 0x8000) == 0 {
        return Err("DNS packet is not a response");
    }
    // Opcode must be standard query (0): bits 14:11
    if (flags & 0x7800) != 0 {
        return Err("DNS non-standard opcode");
    }
    // TC bit must be 0 (not truncated): bit 9
    if (flags & 0x0200) != 0 {
        return Err("DNS response truncated");
    }
    let qd = u16::from_be_bytes([packet[4], packet[5]]);
    if qd == 0 {
        return Err("DNS response missing question");
    }
    if qd != 1 {
        return Err("DNS response QDCOUNT must be 1");
    }

    let mut p = DNS_HEADER_LEN;
    let mut name_offset = 0usize;

    // Decode and verify labels
    loop {
        if p >= packet.len() {
            return Err("truncated DNS question name");
        }
        let label_len = packet[p] as usize;
        p += 1;
        if label_len == 0 {
            break;
        }
        if (label_len & 0xC0) != 0 || p + label_len > packet.len() {
            return Err("invalid DNS label in question");
        }
        if name_offset > 0 {
            if name_offset >= expected_name.len() || expected_name[name_offset] != b'.' {
                return Err("DNS question name mismatch");
            }
            name_offset += 1;
        }
        let label = &packet[p..p + label_len];
        if name_offset + label_len > expected_name.len()
            || !expected_name[name_offset..name_offset + label_len].eq_ignore_ascii_case(label)
        {
            return Err("DNS question name mismatch");
        }
        name_offset += label_len;
        p += label_len;
    }

    if name_offset != expected_name.len() {
        return Err("DNS question name length mismatch");
    }

    if p + 4 > packet.len() {
        return Err("truncated DNS question QTYPE/QCLASS");
    }
    let qtype = u16::from_be_bytes([packet[p], packet[p + 1]]);
    let qclass = u16::from_be_bytes([packet[p + 2], packet[p + 3]]);
    if qtype != 1 || qclass != 1 {
        return Err("DNS question is not A / IN");
    }
    Ok(())
}

pub fn parse_a_answer(buf: &[u8], expected_id: u16) -> Result<DnsAnswer, &'static str> {
    if buf.len() < DNS_HEADER_LEN || u16::from_be_bytes([buf[0], buf[1]]) != expected_id {
        return Err("DNS response id mismatch");
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    let answers = u16::from_be_bytes([buf[6], buf[7]]);
    if flags & 0x8000 == 0 || flags & 0x000F != 0 || answers == 0 {
        return Err("DNS response rejected");
    }
    if (flags & 0x7800) != 0 {
        return Err("DNS non-standard opcode");
    }
    if (flags & 0x0200) != 0 {
        return Err("DNS response truncated");
    }
    let qd = u16::from_be_bytes([buf[4], buf[5]]);
    if qd == 0 {
        return Err("DNS response missing question");
    }
    if qd != 1 {
        return Err("DNS response QDCOUNT must be 1");
    }

    // Decode expected question name into dotted ASCII target
    let mut current_target = [0u8; DNS_MAX_NAME];
    let (qname_consumed, mut target_len) = decode_wire_name(buf, DNS_HEADER_LEN, &mut current_target)?;
    let mut p = DNS_HEADER_LEN + qname_consumed;
    if p + 4 > buf.len() {
        return Err("truncated DNS question");
    }
    p += 4; // Skip QTYPE and QCLASS

    let mut cname_hops = 0usize;
    for _ in 0..answers {
        if p >= buf.len() { return Err("truncated DNS answer"); }
        let mut ans_name = [0u8; DNS_MAX_NAME];
        let (name_consumed, ans_len) = decode_wire_name(buf, p, &mut ans_name)?;
        p += name_consumed;

        if !ans_name[..ans_len].eq_ignore_ascii_case(&current_target[..target_len]) {
            return Err("DNS answer name mismatch");
        }
        if p + 10 > buf.len() { return Err("truncated DNS RR"); }
        let qtype = u16::from_be_bytes([buf[p], buf[p+1]]);
        let class = u16::from_be_bytes([buf[p+2], buf[p+3]]);
        let ttl = u32::from_be_bytes([buf[p+4], buf[p+5], buf[p+6], buf[p+7]]);
        let rdlen = u16::from_be_bytes([buf[p+8], buf[p+9]]) as usize;
        p += 10;
        if p + rdlen > buf.len() { return Err("truncated DNS rdata"); }

        if qtype == 5 && class == 1 {
            // CNAME RR: follow alias to canonical name
            if cname_hops >= DNS_MAX_CNAME_HOPS {
                return Err("excessive CNAME chaining");
            }
            let mut next_target = [0u8; DNS_MAX_NAME];
            let (_, next_len) = decode_wire_name(buf, p, &mut next_target)?;
            current_target = next_target;
            target_len = next_len;
            cname_hops += 1;
            p += rdlen;
            continue;
        }

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
    pub fn cached(&self, name: &[u8], now: u64) -> Option<Ipv4Address> {
        self.cache.iter().find(|e| {
            e.valid
                && e.expires_at > now
                && e.name_len == name.len()
                && e.name[..e.name_len].eq_ignore_ascii_case(name)
        }).map(|e| e.addr)
    }
    pub fn begin(&mut self,name:&[u8],now:u64)->Result<u16,&'static str>{
        if name.is_empty()||name.len()>DNS_MAX_NAME{return Err("invalid DNS name")}
        if self.cached(name,now).is_some(){return Err("DNS cache hit")}
        let mut id = self.next_id.max(1);
        let mut free_id = None;
        for _ in 0..u16::MAX {
            if !self.queries.iter().any(|q| q.valid && q.id == id) {
                free_id = Some(id);
                break;
            }
            id = id.wrapping_add(1).max(1);
        }
        let id = free_id.ok_or("DNS query id space exhausted")?;
        self.next_id = id.wrapping_add(1).max(1);

        let q=self.queries.iter_mut().find(|q|!q.valid).ok_or("DNS query table full")?;
        q.id=id;q.name[..name.len()].copy_from_slice(name);q.name_len=name.len();q.retries=0;q.deadline=now.saturating_add(DNS_RETRY_TICKS);q.valid=true;Ok(id)
    }
    pub fn abort(&mut self, id: u16) -> bool {
        if let Some(q) = self.queries.iter_mut().find(|q| q.valid && q.id == id) {
            q.valid = false;
            q.retries = 0;
            q.deadline = 0;
            return true;
        }
        false
    }
    pub fn build_query(&self,id:u16,out:&mut[u8])->Result<usize,&'static str>{let q=self.queries.iter().find(|q|q.valid&&q.id==id).ok_or("unknown DNS query")?;encode_query(id,&q.name[..q.name_len],out)}

    /// Accepts and integrates a DNS response with strict question-to-query association.
    pub fn accept_response(&mut self, packet: &[u8], now: u64) -> Result<Ipv4Address, &'static str> {
        if packet.len() < DNS_HEADER_LEN { return Err("DNS response too small"); }
        let id = u16::from_be_bytes([packet[0], packet[1]]);
        let q = self.queries.iter_mut().find(|q| q.valid && q.id == id).ok_or("unknown DNS query")?;

        // Anti-Spoofing Defense: Verify the question in the response matches the outstanding query's domain!
        verify_dns_question_matches_query(packet, &q.name[..q.name_len])?;

        let a = parse_a_answer(packet, id)?;
        let mut e = DnsCacheEntry::empty();
        e.name[..q.name_len].copy_from_slice(&q.name[..q.name_len]);
        e.name_len = q.name_len;
        e.addr = a.addr;
        e.expires_at = now.saturating_add((a.ttl as u64).max(1) * 1000);
        e.valid = true;
        if a.ttl > 0 {
            if let Some(dst) = self.cache.iter_mut().find(|e| !e.valid || e.expires_at <= now) {
                *dst = e;
            } else {
                self.cache[0] = e;
            }
        }
        q.valid = false;
        Ok(a.addr)
    }

    /// Accepts and validates a DNS response ensuring sender is the authoritative upstream DNS server.
    pub fn accept_response_from(
        &mut self,
        packet: &[u8],
        sender_ip: Ipv4Address,
        sender_port: u16,
        now: u64,
    ) -> Result<Ipv4Address, &'static str> {
        if sender_ip != DNS_SERVER || sender_port != 53 {
            return Err("DNS response sender mismatch");
        }
        self.accept_response(packet, now)
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


#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn abort_releases_inflight_query() {
        let mut r = DnsResolver::new();
        let id = r.begin(b"example.com", 0).unwrap();
        assert!(r.query(id).is_some());
        assert!(r.abort(id));
        assert!(r.query(id).is_none());
        let replacement = r.begin(b"example.net", 1).unwrap();
        assert_ne!(replacement, 0);
    }

    #[test]
    fn query_ids_skip_active_ids_after_wrap() {
        let mut r = DnsResolver::new();
        r.next_id = u16::MAX;
        let first = r.begin(b"one.example", 0).unwrap();
        let second = r.begin(b"two.example", 0).unwrap();
        assert_ne!(first, second);
        assert_ne!(second, 0);
    }

    #[test]
    fn dns_answer_name_mismatch_rejected() {
        // Construct a DNS response packet for example.com with ID 0x1234
        // Header: id=0x1234, flags=0x8180 (std response, no error), qdcount=1, ancount=1, nscount=0, arcount=0
        let mut packet = alloc::vec![
            0x12, 0x34, 0x81, 0x80,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
        ];
        // Question: \x07example\x03com\x00, qtype=1 (A), qclass=1 (IN)
        packet.extend_from_slice(b"\x07example\x03com\x00\x00\x01\x00\x01");

        // Valid answer using compression pointer to question name (0xC00C)
        let mut valid_packet = packet.clone();
        valid_packet.extend_from_slice(&[
            0xC0, 0x0C,             // Pointer to byte 12 (question name)
            0x00, 0x01, 0x00, 0x01, // qtype=A, qclass=IN
            0x00, 0x00, 0x00, 0x3C, // TTL = 60s
            0x00, 0x04,             // rdlen = 4
            192, 0, 2, 1,           // IPv4 address 192.0.2.1
        ]);
        let ans = parse_a_answer(&valid_packet, 0x1234).expect("valid answer accepted");
        assert_eq!(ans.addr, Ipv4Address([192, 0, 2, 1]));

        // Poisoned answer with bad pointer to non-question offset
        let mut bad_ptr_packet = packet.clone();
        bad_ptr_packet.extend_from_slice(&[
            0xC0, 0x00,             // Pointer to byte 0 (not question name!)
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x3C,
            0x00, 0x04,
            192, 0, 2, 1,
        ]);
        assert_eq!(
            parse_a_answer(&bad_ptr_packet, 0x1234),
            Err("DNS answer name pointer mismatch")
        );

        // Poisoned answer with mismatched literal name
        let mut bad_name_packet = packet.clone();
        bad_name_packet.extend_from_slice(b"\x06evilco\x03com\x00");
        bad_name_packet.extend_from_slice(&[
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x3C,
            0x00, 0x04,
            198, 51, 100, 1,
        ]);
        assert_eq!(
            parse_a_answer(&bad_name_packet, 0x1234),
            Err("DNS answer name mismatch")
        );
    }

    #[test]
    fn accept_response_rejects_question_spoofing_and_unauthorized_server() {
        let mut resolver = DnsResolver::new();
        let query_id = resolver.begin(b"legit.example.com", 0).unwrap();

        // 1. Construct response with matching query_id, but spoofed question "attacker.com"
        let mut spoofed_packet = alloc::vec![
            (query_id >> 8) as u8, (query_id & 0xFF) as u8, 0x81, 0x80,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
        ];
        // Question: \x08attacker\x03com\x00, qtype=1, qclass=1
        spoofed_packet.extend_from_slice(b"\x08attacker\x03com\x00\x00\x01\x00\x01");
        // Answer for attacker.com
        spoofed_packet.extend_from_slice(&[
            0xC0, 0x0C,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x3C,
            0x00, 0x04,
            6, 6, 6, 6,
        ]);

        // Attempt to accept response: MUST BE REJECTED because question doesn't match "legit.example.com"
        let res = resolver.accept_response(&spoofed_packet, 10);
        assert_eq!(res, Err("DNS question name mismatch"));

        // 2. Construct response with legitimate question "legit.example.com"
        let mut legit_packet = alloc::vec![
            (query_id >> 8) as u8, (query_id & 0xFF) as u8, 0x81, 0x80,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
        ];
        // Question: \x05legit\x07example\x03com\x00
        legit_packet.extend_from_slice(b"\x05legit\x07example\x03com\x00\x00\x01\x00\x01");
        legit_packet.extend_from_slice(&[
            0xC0, 0x0C,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x3C,
            0x00, 0x04,
            93, 184, 216, 34,
        ]);

        // 3. Test accept_response_from with wrong IP or wrong port
        let wrong_ip = Ipv4Address([10, 0, 2, 4]);
        assert_eq!(
            resolver.accept_response_from(&legit_packet, wrong_ip, 53, 20),
            Err("DNS response sender mismatch")
        );
        assert_eq!(
            resolver.accept_response_from(&legit_packet, DNS_SERVER, 5353, 20),
            Err("DNS response sender mismatch")
        );

        // 4. Test accept_response_from with valid DNS_SERVER:53
        let accepted = resolver.accept_response_from(&legit_packet, DNS_SERVER, 53, 20).unwrap();
        assert_eq!(accepted, Ipv4Address([93, 184, 216, 34]));
        // Cached value lookup
        assert_eq!(resolver.cached(b"legit.example.com", 25), Some(Ipv4Address([93, 184, 216, 34])));
    }

    #[test]
    fn dns_case_insensitive_question_and_answer_matching() {
        let mut r = DnsResolver::new();
        let query_id = r.begin(b"example.com", 0).unwrap();

        // 1. Packet echoes question in UPPERCASE ("EXAMPLE.COM")
        let mut packet = alloc::vec![
            (query_id >> 8) as u8, (query_id & 0xFF) as u8, 0x81, 0x80,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
        ];
        packet.extend_from_slice(b"\x07EXAMPLE\x03COM\x00\x00\x01\x00\x01");
        // Answer echoes name in mixed case ("eXaMpLe.CoM")
        packet.extend_from_slice(b"\x07eXaMpLe\x03CoM\x00\x00\x01\x00\x01\x00\x00\x00\x3C\x00\x04");
        packet.extend_from_slice(&[93, 184, 216, 34]);

        let resolved = r.accept_response_from(&packet, DNS_SERVER, 53, 10)
            .expect("case-insensitive response accepted");
        assert_eq!(resolved, Ipv4Address([93, 184, 216, 34]));

        // 2. Cache hit with mixed-case query
        assert_eq!(r.cached(b"ExAmPlE.cOm", 15), Some(Ipv4Address([93, 184, 216, 34])));
        assert_eq!(r.cached(b"EXAMPLE.COM", 15), Some(Ipv4Address([93, 184, 216, 34])));
    }

    #[test]
    fn dns_header_flags_and_qdcount_validation() {
        // Base packet for test.com
        let base_hdr = |qr_bit: u16, opcode: u16, tc_bit: u16, qd: u16, an: u16| {
            let mut p = alloc::vec![0x12, 0x34];
            let flags = (qr_bit << 15) | (opcode << 11) | (tc_bit << 9) | 0x0180;
            p.extend_from_slice(&flags.to_be_bytes());
            p.extend_from_slice(&qd.to_be_bytes());
            p.extend_from_slice(&an.to_be_bytes());
            p.extend_from_slice(&[0, 0, 0, 0]); // NS, AR
            p
        };

        // Reject Query packet (QR = 0)
        let mut qr_zero = base_hdr(0, 0, 0, 1, 1);
        qr_zero.extend_from_slice(b"\x04test\x03com\x00\x00\x01\x00\x01");
        assert_eq!(verify_dns_question_matches_query(&qr_zero, b"test.com"), Err("DNS packet is not a response"));

        // Reject Non-Standard Opcode (Opcode = 1: IQUERY)
        let mut bad_opcode = base_hdr(1, 1, 0, 1, 1);
        bad_opcode.extend_from_slice(b"\x04test\x03com\x00\x00\x01\x00\x01");
        assert_eq!(verify_dns_question_matches_query(&bad_opcode, b"test.com"), Err("DNS non-standard opcode"));

        // Reject Truncated packet (TC = 1)
        let mut truncated = base_hdr(1, 0, 1, 1, 1);
        truncated.extend_from_slice(b"\x04test\x03com\x00\x00\x01\x00\x01");
        assert_eq!(verify_dns_question_matches_query(&truncated, b"test.com"), Err("DNS response truncated"));

        // Reject Multiple Questions (QDCOUNT = 2)
        let mut qd_two = base_hdr(1, 0, 0, 2, 1);
        qd_two.extend_from_slice(b"\x04test\x03com\x00\x00\x01\x00\x01");
        assert_eq!(verify_dns_question_matches_query(&qd_two, b"test.com"), Err("DNS response QDCOUNT must be 1"));
    }

    #[test]
    fn dns_compression_pointer_security_bounds() {
        // Construct standard header and question at offset 12
        let mut base = alloc::vec![
            0xAA, 0xBB, 0x81, 0x80,
            0x00, 0x01, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
        ];
        base.extend_from_slice(b"\x04loop\x03com\x00\x00\x01\x00\x01");
        let ans_offset = base.len();

        // 1. Self-referential loop: pointer pointing to itself (ans_offset)
        let mut loop_pkt = base.clone();
        loop_pkt.push(0xC0 | ((ans_offset >> 8) as u8));
        loop_pkt.push((ans_offset & 0xFF) as u8);
        loop_pkt.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3C, 0x00, 0x04, 1, 1, 1, 1]);
        assert_eq!(parse_a_answer(&loop_pkt, 0xAABB), Err("DNS answer name pointer mismatch"));

        // 2. Forward pointer: pointer pointing past itself
        let mut fwd_pkt = base.clone();
        let fwd_target = ans_offset + 10;
        fwd_pkt.push(0xC0 | ((fwd_target >> 8) as u8));
        fwd_pkt.push((fwd_target & 0xFF) as u8);
        fwd_pkt.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3C, 0x00, 0x04, 1, 1, 1, 1]);
        assert_eq!(parse_a_answer(&fwd_pkt, 0xAABB), Err("DNS answer name pointer mismatch"));

        // 3. Out-of-bounds pointer into DNS header (offset 4 < 12)
        let mut hdr_pkt = base.clone();
        hdr_pkt.extend_from_slice(&[0xC0, 0x04]);
        hdr_pkt.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3C, 0x00, 0x04, 1, 1, 1, 1]);
        assert_eq!(parse_a_answer(&hdr_pkt, 0xAABB), Err("DNS answer name pointer mismatch"));
    }

    #[test]
    fn dns_cname_chain_resolution_and_bailiwick() {
        let mut r = DnsResolver::new();
        let query_id = r.begin(b"alias.example.com", 0).unwrap();

        // Response with 2 answers:
        // Answer 1: alias.example.com -> CNAME canonical.example.com
        // Answer 2: canonical.example.com -> A 93.184.216.34
        let mut packet = alloc::vec![
            (query_id >> 8) as u8, (query_id & 0xFF) as u8, 0x81, 0x80,
            0x00, 0x01, 0x00, 0x02, // QD=1, AN=2
            0x00, 0x00, 0x00, 0x00,
        ];
        // Question: alias.example.com
        packet.extend_from_slice(b"\x05alias\x07example\x03com\x00\x00\x01\x00\x01");

        // Answer 1: pointer to question (0xC00C), TYPE=CNAME(5), CLASS=1, TTL=60, RDLEN=21
        packet.extend_from_slice(&[0xC0, 0x0C]);
        packet.extend_from_slice(&[0x00, 0x05, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3C]);
        // RDATA: \x09canonical\x07example\x03com\x00 (length = 23 bytes)
        packet.extend_from_slice(&[0x00, 0x17]); // rdlen = 23
        let cname_rdata_offset = packet.len();
        packet.extend_from_slice(b"\x09canonical\x07example\x03com\x00");

        // Answer 2: Pointer to canonical name in CNAME RDATA, TYPE=A(1), CLASS=1, TTL=60, RDLEN=4
        packet.push(0xC0 | ((cname_rdata_offset >> 8) as u8));
        packet.push((cname_rdata_offset & 0xFF) as u8);
        packet.extend_from_slice(&[0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3C, 0x00, 0x04]);
        packet.extend_from_slice(&[93, 184, 216, 34]);

        let resolved = r.accept_response_from(&packet, DNS_SERVER, 53, 100)
            .expect("CNAME alias chained to A record");
        assert_eq!(resolved, Ipv4Address([93, 184, 216, 34]));

        // Poisoned Answer 2 targeting unrelated bailiwick (evil.com) must be rejected
        let mut poisoned = packet.clone();
        let ans2_idx = poisoned.len() - 16;
        // Overwrite Answer 2 name pointer to point to question name instead of canonical target
        poisoned[ans2_idx] = 0xC0;
        poisoned[ans2_idx + 1] = 0x0C;
        assert_eq!(parse_a_answer(&poisoned, query_id), Err("DNS answer name mismatch"));
    }
}
