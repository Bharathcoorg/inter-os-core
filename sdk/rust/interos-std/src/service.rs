//! Userspace representation of the trusted InterOS service ABI.
//!
//! The wire format is intentionally fixed-width so Ring-3 services can be
//! implemented without heap allocation or serialization dependencies.

pub const MAGIC: [u8; 4] = *b"ISVC";
pub const VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 16;

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
}
