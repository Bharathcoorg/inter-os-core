//! Minimal no_std TCP wire/state boundary for InterOS.
//! This module intentionally keeps transport state separate from the NIC and
//! IPv4 layers. It provides bounded parsing/serialization and the core
//! connection state machine needed by the userspace network service.

pub const TCP_HEADER_LEN: usize = 20;
pub const TCP_MAX_SEGMENT: usize = 1460;

/// RFC 793 sequence-space comparison. Wrapping arithmetic is required because
/// TCP sequence numbers are modulo 2^32.
pub fn seq_lt(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) < 0 }
pub fn seq_le(a: u32, b: u32) -> bool { a == b || seq_lt(a, b) }
pub fn seq_gt(a: u32, b: u32) -> bool { seq_lt(b, a) }
pub fn seq_ge(a: u32, b: u32) -> bool { a == b || seq_gt(a, b) }

pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0usize;
    while i + 1 < data.len() { sum = sum.wrapping_add(u16::from_be_bytes([data[i], data[i + 1]]) as u32); i += 2; }
    if i < data.len() { sum = sum.wrapping_add((data[i] as u32) << 8); }
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    !(sum as u16)
}

pub fn tcp_checksum(src: [u8; 4], dst: [u8; 4], segment: &[u8]) -> u16 {
    let mut pseudo = [0u8; 12 + TCP_HEADER_LEN + TCP_MAX_SEGMENT];
    pseudo[0..4].copy_from_slice(&src);
    pseudo[4..8].copy_from_slice(&dst);
    pseudo[9] = 6;
    let len = segment.len().min(TCP_HEADER_LEN + TCP_MAX_SEGMENT);
    pseudo[10..12].copy_from_slice(&(len as u16).to_be_bytes());
    pseudo[12..12 + len].copy_from_slice(&segment[..len]);
    internet_checksum(&pseudo[..12 + len])
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait,
    CloseWait,
    LastAck,
    TimeWait,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TcpSegment<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub flags: u16,
    pub window: u16,
    pub payload: &'a [u8],
}

impl<'a> TcpSegment<'a> {
    pub fn parse(buf: &'a [u8]) -> Result<Self, &'static str> {
        if buf.len() < TCP_HEADER_LEN { return Err("TCP header too small"); }
        let data_offset = ((buf[12] >> 4) as usize) * 4;
        if data_offset < TCP_HEADER_LEN || data_offset > buf.len() {
            return Err("invalid TCP data offset");
        }
        Ok(Self {
            src_port: u16::from_be_bytes([buf[0], buf[1]]),
            dst_port: u16::from_be_bytes([buf[2], buf[3]]),
            seq: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            ack: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
            flags: u16::from_be_bytes([buf[12] & 0x01, buf[13]]),
            window: u16::from_be_bytes([buf[14], buf[15]]),
            payload: &buf[data_offset..],
        })
    }

    pub fn verify_checksum(&self, src: [u8; 4], dst: [u8; 4], raw: &[u8]) -> bool {
        if raw.len() < TCP_HEADER_LEN || raw.len() > TCP_HEADER_LEN + TCP_MAX_SEGMENT { return false; }
        tcp_checksum(src, dst, raw) == 0
    }

    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if self.payload.len() > TCP_MAX_SEGMENT || out.len() < TCP_HEADER_LEN + self.payload.len() {
            return Err("TCP segment too large");
        }
        out[..TCP_HEADER_LEN].fill(0);
        out[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        out[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        out[4..8].copy_from_slice(&self.seq.to_be_bytes());
        out[8..12].copy_from_slice(&self.ack.to_be_bytes());
        out[12] = 0x50 | ((self.flags >> 8) as u8 & 0x01);
        out[13] = self.flags as u8;
        out[14..16].copy_from_slice(&self.window.to_be_bytes());
        out[16..18].copy_from_slice(&0u16.to_be_bytes());
        out[18..20].copy_from_slice(&0u16.to_be_bytes());
        out[TCP_HEADER_LEN..TCP_HEADER_LEN + self.payload.len()].copy_from_slice(self.payload);
        Ok(TCP_HEADER_LEN + self.payload.len())
    }
}

pub fn serialize_with_checksum(
    segment: &TcpSegment<'_>,
    src: [u8; 4],
    dst: [u8; 4],
    out: &mut [u8],
) -> Result<usize, &'static str> {
    let n = segment.serialize(out)?;
    out[16..18].fill(0);
    let checksum = tcp_checksum(src, dst, &out[..n]);
    out[16..18].copy_from_slice(&checksum.to_be_bytes());
    Ok(n)
}

pub fn next_state(state: TcpState, flags: u16) -> TcpState {
    const SYN: u16 = 0x02;
    const ACK: u16 = 0x10;
    const FIN: u16 = 0x01;
    match state {
        TcpState::Closed if flags & SYN != 0 => TcpState::SynReceived,
        TcpState::Listen if flags & SYN != 0 => TcpState::SynReceived,
        TcpState::SynSent if flags & SYN != 0 && flags & ACK != 0 => TcpState::Established,
        TcpState::SynReceived if flags & ACK != 0 => TcpState::Established,
        TcpState::Established if flags & FIN != 0 => TcpState::CloseWait,
        TcpState::FinWait if flags & ACK != 0 => TcpState::Closed,
        TcpState::LastAck if flags & ACK != 0 => TcpState::Closed,
        TcpState::TimeWait => TcpState::TimeWait,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tcp_checksum_detects_tamper() {
        let seg = TcpSegment { src_port: 1000, dst_port: 443, seq: 1, ack: 2, flags: 0x18, window: 4096, payload: b"hello" };
        let mut raw = [0u8; 64];
        let n = seg.serialize(&mut raw).unwrap();
        let csum = tcp_checksum([10,0,0,1], [10,0,0,2], &raw[..n]);
        raw[16..18].copy_from_slice(&csum.to_be_bytes());
        assert_eq!(tcp_checksum([10,0,0,1], [10,0,0,2], &raw[..n]), 0);
        raw[n-1] ^= 1;
        assert_ne!(tcp_checksum([10,0,0,1], [10,0,0,2], &raw[..n]), 0);
    }

    #[test]
    fn tcp_round_trip() {
        let seg = TcpSegment { src_port: 1234, dst_port: 443, seq: 10, ack: 20, flags: 0x18, window: 4096, payload: b"GET" };
        let mut raw = [0u8; 64];
        let n = seg.serialize(&mut raw).unwrap();
        let parsed = TcpSegment::parse(&raw[..n]).unwrap();
        assert_eq!(parsed.src_port, 1234);
        assert_eq!(parsed.dst_port, 443);
        assert_eq!(parsed.seq, 10);
        assert_eq!(parsed.payload, b"GET");
    }
}


#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Retransmission { pub seq:u32, pub len:u16, pub sent_at:u64, pub attempts:u8, pub rto:u64 }

pub const MAX_RETRANSMISSIONS: usize = 16;
pub const TCP_RETRANSMIT_TIMEOUT: u64 = 100;
pub const TCP_RETRANSMIT_MAX_TIMEOUT: u64 = 1600;
pub const TCP_TIME_WAIT: u64 = 200;
pub const TCP_MAX_RETRANSMISSIONS: u8 = 8;

pub struct TcpReliability { pending:[Option<Retransmission>;MAX_RETRANSMISSIONS], pub time_wait_until:u64 }
impl TcpReliability {
 pub const fn new()->Self{Self{pending:[None;MAX_RETRANSMISSIONS],time_wait_until:0}}
 pub fn track(&mut self,seq:u32,len:u16,now:u64)->Result<(),&'static str>{
  if let Some(slot)=self.pending.iter_mut().find(|x|x.is_none()){*slot=Some(Retransmission{seq,len,sent_at:now,attempts:0,rto:TCP_RETRANSMIT_TIMEOUT});Ok(())}else{Err("TCP retransmission queue full")}
 }
 pub fn acknowledge(&mut self,ack:u32){for slot in &mut self.pending{if let Some(r)=*slot{if seq_ge(ack,r.seq.wrapping_add(r.len as u32)){*slot=None;}}}}
 pub fn due(&mut self,now:u64)->Option<Retransmission>{for slot in &mut self.pending{if let Some(mut r)=*slot{if now.saturating_sub(r.sent_at)>=r.rto{if r.attempts>=TCP_MAX_RETRANSMISSIONS{*slot=None;return None;}r.sent_at=now;r.attempts=r.attempts.saturating_add(1);r.rto=(r.rto.saturating_mul(2)).min(TCP_RETRANSMIT_MAX_TIMEOUT);*slot=Some(r);return Some(r);}}}None}
 pub fn enter_time_wait(&mut self,now:u64){self.time_wait_until=now.saturating_add(TCP_TIME_WAIT)}
 pub fn time_wait_expired(&self,now:u64)->bool{self.time_wait_until!=0&&now>=self.time_wait_until}
 pub fn in_time_wait(&self,now:u64)->bool{self.time_wait_until!=0&&now<self.time_wait_until}
 pub fn pending(&self)->usize{self.pending.iter().filter(|x|x.is_some()).count()}
}
