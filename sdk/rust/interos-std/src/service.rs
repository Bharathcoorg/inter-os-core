//! Userspace representation of the trusted InterOS service ABI.
//!
//! The wire format is intentionally fixed-width so Ring-3 services can be
//! implemented without heap allocation or serialization dependencies.

pub const MAGIC: [u8; 4] = *b"ISVC";
pub const VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 16;
pub const MAX_PAYLOAD: u16 = 4096;

#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Register = 1,
    Heartbeat = 2,
    Query = 3,
    Event = 4,
    Shutdown = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub op: Op,
    pub service_id: u8,
    pub flags: u8,
    pub request_id: u32,
    pub payload_len: u16,
}

impl Message {
    pub fn encode(self, out: &mut [u8; HEADER_SIZE]) {
        out[..4].copy_from_slice(&MAGIC);
        out[4..6].copy_from_slice(&VERSION.to_le_bytes());
        out[6..8].copy_from_slice(&(self.op as u16).to_le_bytes());
        out[8] = self.service_id;
        out[9] = self.flags;
        out[10..14].copy_from_slice(&self.request_id.to_le_bytes());
        out[14..16].copy_from_slice(&self.payload_len.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_SIZE || buf[..4] != MAGIC { return None; }
        if u16::from_le_bytes([buf[4], buf[5]]) != VERSION { return None; }
        let op = match u16::from_le_bytes([buf[6], buf[7]]) {
            1 => Op::Register, 2 => Op::Heartbeat, 3 => Op::Query,
            4 => Op::Event, 5 => Op::Shutdown, _ => return None,
        };
        let service_id = buf[8];
        if !(1..=11).contains(&service_id) { return None; }
        let payload_len = u16::from_le_bytes([buf[14], buf[15]]);
        if payload_len > MAX_PAYLOAD { return None; }
        Some(Self {
            op,
            service_id,
            flags: buf[9],
            request_id: u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]),
            payload_len,
        })
    }
}
