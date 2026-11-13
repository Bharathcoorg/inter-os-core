// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Userspace representation of the trusted InterOS service ABI.
//!
//! The wire format is intentionally fixed-width so Ring-3 services can be
//! implemented without heap allocation or serialization dependencies.

pub const MAGIC: [u8; 4] = *b"ISVC";
pub const VERSION: u16 = 1;
pub const HEADER_SIZE: usize = 16;
pub const MAX_PAYLOAD: u16 = 4096;
pub const SERVICE_ID_MIN: u8 = 1;
pub const SERVICE_ID_MAX: u8 = 15;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceId {
    Display = 1,
    Input = 2,
    Storage = 3,
    Network = 4,
    Power = 5,
    Modem = 6,
    Audio = 7,
    Camera = 8,
    Compositor = 9,
    Launcher = 10,
    Sensor = 11,
    Inference = 12,
    Search = 13,
    Agent = 14,
    Voice = 15,
}

impl ServiceId {
    pub const fn from_u8(id: u8) -> Option<Self> {
        match id {
            1 => Some(ServiceId::Display),
            2 => Some(ServiceId::Input),
            3 => Some(ServiceId::Storage),
            4 => Some(ServiceId::Network),
            5 => Some(ServiceId::Power),
            6 => Some(ServiceId::Modem),
            7 => Some(ServiceId::Audio),
            8 => Some(ServiceId::Camera),
            9 => Some(ServiceId::Compositor),
            10 => Some(ServiceId::Launcher),
            11 => Some(ServiceId::Sensor),
            12 => Some(ServiceId::Inference),
            13 => Some(ServiceId::Search),
            14 => Some(ServiceId::Agent),
            15 => Some(ServiceId::Voice),
            _ => None,
        }
    }
}

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
        if buf.len() < HEADER_SIZE || buf[..4] != MAGIC {
            return None;
        }
        if u16::from_le_bytes([buf[4], buf[5]]) != VERSION {
            return None;
        }
        let op = match u16::from_le_bytes([buf[6], buf[7]]) {
            1 => Op::Register,
            2 => Op::Heartbeat,
            3 => Op::Query,
            4 => Op::Event,
            5 => Op::Shutdown,
            _ => return None,
        };
        let service_id = buf[8];
        if !(SERVICE_ID_MIN..=SERVICE_ID_MAX).contains(&service_id) {
            return None;
        }
        let payload_len = u16::from_le_bytes([buf[14], buf[15]]);
        if payload_len > MAX_PAYLOAD {
            return None;
        }
        Some(Self {
            op,
            service_id,
            flags: buf[9],
            request_id: u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]),
            payload_len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userspace_service_abi_parity_lowest_id() {
        let msg = Message {
            op: Op::Register,
            service_id: SERVICE_ID_MIN,
            flags: 0x01,
            request_id: 42,
            payload_len: 16,
        };
        let mut buf = [0u8; HEADER_SIZE];
        msg.encode(&mut buf);
        let decoded = Message::decode(&buf).expect("lowest ID must decode in userspace");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn userspace_service_abi_parity_highest_id() {
        let msg = Message {
            op: Op::Shutdown,
            service_id: SERVICE_ID_MAX,
            flags: 0,
            request_id: 99,
            payload_len: 0,
        };
        let mut buf = [0u8; HEADER_SIZE];
        msg.encode(&mut buf);
        let decoded = Message::decode(&buf).expect("highest ID must decode in userspace");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn userspace_service_abi_supports_inference_id_12() {
        let msg = Message {
            op: Op::Heartbeat,
            service_id: ServiceId::Inference as u8,
            flags: 0,
            request_id: 1,
            payload_len: 0,
        };
        assert_eq!(msg.service_id, 12);
        let mut buf = [0u8; HEADER_SIZE];
        msg.encode(&mut buf);
        let decoded = Message::decode(&buf).expect("inference service ID 12 must decode");
        assert_eq!(decoded, msg);
        assert_eq!(ServiceId::from_u8(12), Some(ServiceId::Inference));
    }

    #[test]
    fn userspace_service_abi_rejects_zero_and_out_of_bounds() {
        let mut buf = [0u8; HEADER_SIZE];
        let msg = Message {
            op: Op::Query,
            service_id: 1,
            flags: 0,
            request_id: 1,
            payload_len: 0,
        };
        msg.encode(&mut buf);

        // ID 0 rejected
        buf[8] = 0;
        assert_eq!(Message::decode(&buf), None);

        // ID 16 rejected
        buf[8] = 16;
        assert_eq!(Message::decode(&buf), None);

        // ID 255 rejected
        buf[8] = 255;
        assert_eq!(Message::decode(&buf), None);
    }

    #[test]
    fn userspace_service_abi_rejects_malformed() {
        let mut buf = [0u8; HEADER_SIZE];
        let msg = Message {
            op: Op::Heartbeat,
            service_id: 1,
            flags: 0,
            request_id: 1,
            payload_len: 0,
        };
        msg.encode(&mut buf);

        // Short buffer
        assert_eq!(Message::decode(&buf[..12]), None);

        // Bad magic
        let mut bad_magic = buf;
        bad_magic[0] = b'Z';
        assert_eq!(Message::decode(&bad_magic), None);

        // Bad version
        let mut bad_version = buf;
        bad_version[4] = 2;
        assert_eq!(Message::decode(&bad_version), None);

        // Bad op
        let mut bad_op = buf;
        bad_op[6] = 99;
        assert_eq!(Message::decode(&bad_op), None);

        // Oversized payload
        let mut bad_payload = buf;
        bad_payload[14..16].copy_from_slice(&5000u16.to_le_bytes());
        assert_eq!(Message::decode(&bad_payload), None);
    }
}
