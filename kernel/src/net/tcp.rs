// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
    FinWait1,
    FinWait2,
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
        let src_port = u16::from_be_bytes([buf[0], buf[1]]);
        let dst_port = u16::from_be_bytes([buf[2], buf[3]]);
        if src_port == 0 || dst_port == 0 {
            return Err("TCP port 0 is invalid");
        }
        Ok(Self {
            src_port,
            dst_port,
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

pub fn valid_syn_ack(segment: &TcpSegment<'_>, expected_ack: u32) -> bool {
    const SYN: u16 = 0x02;
    const ACK: u16 = 0x10;
    const RST: u16 = 0x04;
    segment.flags & RST == 0
        && segment.flags == (SYN | ACK)
        && segment.ack == expected_ack
        && segment.payload.is_empty()
}

pub fn valid_established_segment(
    segment: &TcpSegment<'_>,
    expected_seq: u32,
    snd_una: u32,
    snd_nxt: u32,
) -> bool {
    const SYN: u16 = 0x02;
    const RST: u16 = 0x04;
    const ACK: u16 = 0x10;
    if segment.flags & (SYN | RST) != 0 || segment.flags & ACK == 0 { return false; }
    if seq_gt(segment.seq, expected_seq) { return false; }
    if seq_lt(segment.ack, snd_una) || seq_gt(segment.ack, snd_nxt) { return false; }
    true
}

/// RFC 793 §3.3 segment acceptability: does any byte of this segment fall in
/// the receive window `[recv_seq, recv_seq + rcv_wnd)`?
///
/// `seg_len` counts the FIN as one sequence number, because a bare FIN occupies
/// one. With a zero window only the exact next expected sequence is acceptable,
/// which lets a peer probe a closed window without being able to inject into it.
pub fn seq_in_receive_window(seg_seq: u32, seg_len: u32, recv_seq: u32, rcv_wnd: u32) -> bool {
    if rcv_wnd == 0 {
        return seg_len == 0 && seg_seq == recv_seq;
    }
    let window_end = recv_seq.wrapping_add(rcv_wnd);
    let in_window = |s: u32| seq_ge(s, recv_seq) && seq_lt(s, window_end);
    if seg_len == 0 {
        return in_window(seg_seq);
    }
    in_window(seg_seq) || in_window(seg_seq.wrapping_add(seg_len - 1))
}

/// What to do with a segment carrying RST.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RstAction {
    /// In sequence: the peer really has forgotten this connection.
    Accept,
    /// In window but not at `recv_seq`: answer with our own sequence numbers
    /// rather than tearing down, and let a genuine peer confirm.
    ChallengeAck,
    /// Out of window, or a SYN-SENT reset that does not acknowledge our SYN.
    Drop,
}

/// RFC 5961 §5: a reset is only believable at the exact next expected sequence.
///
/// WHY an in-window reset gets a challenge instead of a teardown: accepting any
/// in-window RST let a single forged segment kill the connection, so an off-path
/// attacker needed only the 4-tuple -- not the 32-bit sequence space -- to abort
/// any transfer. An out-of-window reset is dropped silently, so it cannot be
/// used to make this host emit traffic at a chosen victim.
pub fn rst_action(
    state: TcpState,
    seg_seq: u32,
    seg_ack: u32,
    recv_seq: u32,
    snd_nxt: u32,
    rcv_wnd: u32,
) -> RstAction {
    if state == TcpState::SynSent {
        // No receive window exists yet, so the reset must acknowledge the SYN
        // we sent. `snd_nxt` here is the sequence just past that SYN.
        return if seg_ack == snd_nxt { RstAction::Accept } else { RstAction::Drop };
    }
    if seg_seq == recv_seq {
        return RstAction::Accept;
    }
    if rcv_wnd != 0 && seq_gt(seg_seq, recv_seq) && seq_lt(seg_seq, recv_seq.wrapping_add(rcv_wnd)) {
        return RstAction::ChallengeAck;
    }
    RstAction::Drop
}

/// The states in which a segment must be window- and acknowledgement-validated
/// before it may move the connection or retire a pending FIN.
pub fn is_closing_state(state: TcpState) -> bool {
    matches!(
        state,
        TcpState::FinWait1
            | TcpState::FinWait2
            | TcpState::FinWait
            | TcpState::CloseWait
            | TcpState::LastAck
            | TcpState::TimeWait
    )
}

/// Window and acknowledgement validation for the closing states.
///
/// WHY this is separate from `valid_established_segment`: the closing states
/// used to accept any segment at all, overwriting `recv_seq` from the wire and
/// handing `segment.ack` straight to `acknowledge_fin`, whose only test was a
/// lower bound. A forged ACK of `0xFFFFFFFF` satisfied that bound and closed the
/// connection; a forged sequence rewound the receive stream.
pub fn valid_closing_segment(
    segment: &TcpSegment<'_>,
    recv_seq: u32,
    rcv_wnd: u32,
    snd_una: u32,
    snd_nxt: u32,
) -> bool {
    const SYN: u16 = 0x02;
    const RST: u16 = 0x04;
    const ACK: u16 = 0x10;
    const FIN: u16 = 0x01;
    if segment.flags & (SYN | RST) != 0 || segment.flags & ACK == 0 { return false; }
    let len = segment.payload.len() as u32 + u32::from(segment.flags & FIN != 0);
    if !seq_in_receive_window(segment.seq, len, recv_seq, rcv_wnd) { return false; }
    if seq_lt(segment.ack, snd_una) || seq_gt(segment.ack, snd_nxt) { return false; }
    true
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
        TcpState::FinWait1 if flags & FIN != 0 && flags & ACK != 0 => TcpState::TimeWait,
        TcpState::FinWait1 if flags & ACK != 0 => TcpState::FinWait2,
        TcpState::FinWait1 if flags & FIN != 0 => TcpState::TimeWait,
        TcpState::FinWait2 if flags & FIN != 0 => TcpState::TimeWait,
        TcpState::FinWait if flags & ACK != 0 => TcpState::TimeWait,
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

    #[test]
    fn tcp_retransmission_exhaustion_does_not_starve_remaining_slots() {
        let mut rel = TcpReliability::new();
        rel.track(100, 10, 0).unwrap();
        rel.track(200, 20, 0).unwrap();
        assert_eq!(rel.pending(), 2);

        // Advance slot 0 to the maximum number of attempts
        if let Some(r) = rel.pending[0].as_mut() {
            r.attempts = TCP_MAX_RETRANSMISSIONS;
            r.sent_at = 0;
            r.rto = 100;
        }

        // When poll due occurs at tick 150, slot 0 is exhausted and purged,
        // but due() continues scanning and returns slot 1 (seq 200).
        let retrans = rel.due(150);
        assert!(retrans.is_some(), "Retransmission for slot 1 must not be starved by exhausted slot 0");
        assert_eq!(retrans.unwrap().seq, 200);
        assert_eq!(rel.pending(), 1, "Exhausted slot 0 must be evicted");
    }

    const ACK: u16 = 0x10;
    const RST: u16 = 0x04;
    const FIN: u16 = 0x01;

    fn segment(seq: u32, ack: u32, flags: u16, payload: &'static [u8]) -> TcpSegment<'static> {
        TcpSegment { src_port: 443, dst_port: 41000, seq, ack, flags, window: 4096, payload }
    }

    /// The attack RFC 5961 exists to stop: one forged RST at a sequence the
    /// attacker never had to guess correctly. Before the fix every one of these
    /// tore the connection down.
    #[test]
    fn a_reset_outside_the_receive_window_is_dropped_without_an_answer() {
        let recv_seq = 5000u32;
        let wnd = 4096u32;
        for seq in [0u32, 4999, recv_seq.wrapping_add(wnd), u32::MAX] {
            assert_eq!(
                rst_action(TcpState::Established, seq, 0, recv_seq, 9000, wnd),
                RstAction::Drop,
                "a reset at {seq} is outside [{recv_seq}, {}) and must be dropped",
                recv_seq.wrapping_add(wnd)
            );
        }
    }

    #[test]
    fn a_reset_at_the_next_expected_sequence_is_accepted() {
        assert_eq!(
            rst_action(TcpState::Established, 5000, 0, 5000, 9000, 4096),
            RstAction::Accept
        );
    }

    #[test]
    fn an_in_window_reset_is_challenged_rather_than_believed() {
        for seq in [5001u32, 7000, 9095] {
            assert_eq!(
                rst_action(TcpState::Established, seq, 0, 5000, 9000, 4096),
                RstAction::ChallengeAck,
                "an in-window reset at {seq} must draw a challenge ACK, not a teardown"
            );
        }
    }

    #[test]
    fn a_syn_sent_reset_must_acknowledge_the_syn_we_sent() {
        // The SYN occupied 100, so snd_nxt is 101. Sequence is irrelevant here.
        assert_eq!(rst_action(TcpState::SynSent, 777, 101, 0, 101, 0), RstAction::Accept);
        for ack in [0u32, 100, 102, u32::MAX] {
            assert_eq!(
                rst_action(TcpState::SynSent, 777, ack, 0, 101, 0),
                RstAction::Drop,
                "a reset acknowledging {ack} does not acknowledge our SYN"
            );
        }
    }

    #[test]
    fn a_zero_window_accepts_only_the_exact_next_sequence() {
        assert_eq!(rst_action(TcpState::Established, 5000, 0, 5000, 9000, 0), RstAction::Accept);
        assert_eq!(
            rst_action(TcpState::Established, 5001, 0, 5000, 9000, 0),
            RstAction::Drop,
            "with no window there is nothing to challenge about"
        );
    }

    #[test]
    fn reset_validation_survives_sequence_space_wrap() {
        let recv_seq = 0xFFFF_FFF0u32;
        let wnd = 32u32;
        assert_eq!(rst_action(TcpState::Established, recv_seq, 0, recv_seq, 0, wnd), RstAction::Accept);
        for seq in [0xFFFF_FFF1u32, 0xFFFF_FFFF, 0x0000_000F] {
            assert_eq!(
                rst_action(TcpState::Established, seq, 0, recv_seq, 0, wnd),
                RstAction::ChallengeAck,
                "{seq:#x} is inside the wrapped window"
            );
        }
        assert_eq!(
            rst_action(TcpState::Established, 0x0000_0010, 0, recv_seq, 0, wnd),
            RstAction::Drop,
            "the wrapped window end is exclusive"
        );
    }

    #[test]
    fn a_closing_state_refuses_an_ack_for_bytes_never_sent() {
        let seg = segment(5000, 0, ACK, b"");
        for ack in [99u32, 201, 0x8000_0000, u32::MAX] {
            let forged = TcpSegment { ack, ..seg };
            assert!(
                !valid_closing_segment(&forged, 5000, 4096, 100, 200),
                "ack {ack:#x} is outside [snd_una=100, snd_nxt=200]"
            );
        }
        // Positive control: the boundaries themselves are legitimate.
        for ack in [100u32, 150, 200] {
            let honest = TcpSegment { ack, ..seg };
            assert!(valid_closing_segment(&honest, 5000, 4096, 100, 200), "ack {ack} is in range");
        }
    }

    #[test]
    fn a_closing_state_refuses_a_segment_outside_the_receive_window() {
        for seq in [0u32, 4999, 9096, u32::MAX] {
            let seg = segment(seq, 150, ACK, b"x");
            assert!(
                !valid_closing_segment(&seg, 5000, 4096, 100, 200),
                "sequence {seq} must not be allowed to move recv_seq"
            );
        }
        let in_order = segment(5000, 150, ACK, b"payload");
        assert!(valid_closing_segment(&in_order, 5000, 4096, 100, 200));
    }

    #[test]
    fn a_closing_state_counts_the_fin_as_one_sequence_number() {
        // Last byte of a bare FIN lands on the final in-window sequence.
        let at_edge = segment(9095, 150, ACK | FIN, b"");
        assert!(valid_closing_segment(&at_edge, 5000, 4096, 100, 200));
        let past_edge = segment(9096, 150, ACK | FIN, b"");
        assert!(!valid_closing_segment(&past_edge, 5000, 4096, 100, 200));
        // A zero-window peer may still be probed at exactly recv_seq.
        let probe = segment(5000, 150, ACK, b"");
        assert!(valid_closing_segment(&probe, 5000, 0, 100, 200));
        let inject = segment(5000, 150, ACK, b"x");
        assert!(!valid_closing_segment(&inject, 5000, 0, 100, 200));
    }

    #[test]
    fn closing_validation_refuses_syn_and_rst_bearing_segments() {
        for flags in [0x02u16, ACK | 0x02, RST, ACK | RST, FIN] {
            let seg = segment(5000, 150, flags, b"");
            assert!(
                !valid_closing_segment(&seg, 5000, 4096, 100, 200),
                "flags {flags:#x} must not drive a closing state"
            );
        }
    }

    #[test]
    fn the_closing_state_set_covers_every_post_established_state() {
        for state in [
            TcpState::FinWait1, TcpState::FinWait2, TcpState::FinWait,
            TcpState::CloseWait, TcpState::LastAck, TcpState::TimeWait,
        ] {
            assert!(is_closing_state(state), "{state:?} must be window-validated");
        }
        for state in [TcpState::Closed, TcpState::Listen, TcpState::SynSent, TcpState::SynReceived, TcpState::Established] {
            assert!(!is_closing_state(state), "{state:?} is not a closing state");
        }
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
 pub fn due(&mut self,now:u64)->Option<Retransmission>{for slot in &mut self.pending{if let Some(mut r)=*slot{if now.saturating_sub(r.sent_at)>=r.rto{if r.attempts>=TCP_MAX_RETRANSMISSIONS{*slot=None;continue;}r.sent_at=now;r.attempts=r.attempts.saturating_add(1);r.rto=(r.rto.saturating_mul(2)).min(TCP_RETRANSMIT_MAX_TIMEOUT);*slot=Some(r);return Some(r);}}}None}
 pub fn enter_time_wait(&mut self,now:u64){self.time_wait_until=now.saturating_add(TCP_TIME_WAIT)}
 pub fn time_wait_expired(&self,now:u64)->bool{self.time_wait_until!=0&&now>=self.time_wait_until}
 pub fn in_time_wait(&self,now:u64)->bool{self.time_wait_until!=0&&now<self.time_wait_until}
 pub fn pending(&self)->usize{self.pending.iter().filter(|x|x.is_some()).count()}
}
