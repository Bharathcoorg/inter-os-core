// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Trusted userspace service ABI.
//!
//! Narrow wire contract between PID1/service-manager and future Ring-3
//! system services. Lifecycle authority remains in PID1; services do not
//! receive generic SystemControl authority.

#![allow(dead_code)]

pub const SERVICE_ABI_MAGIC: [u8; 4] = *b"ISVC";
pub const SERVICE_ABI_VERSION: u16 = 1;
pub const SERVICE_ABI_HEADER_SIZE: usize = 16;
pub const SERVICE_ABI_MAX_PAYLOAD: u16 = 4096;
pub const SERVICE_ID_MIN: u8 = 1;
pub const SERVICE_ID_MAX: u8 = 15;

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceOp {
    Register = 1,
    Heartbeat = 2,
    Query = 3,
    Event = 4,
    Shutdown = 5,
}

impl ServiceOp {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Register),
            2 => Some(Self::Heartbeat),
            3 => Some(Self::Query),
            4 => Some(Self::Event),
            5 => Some(Self::Shutdown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceMessage {
    pub op: ServiceOp,
    pub service_id: u8,
    pub flags: u8,
    pub request_id: u32,
    pub payload_len: u16,
}

impl ServiceMessage {
    pub fn encode(self, out: &mut [u8; SERVICE_ABI_HEADER_SIZE]) {
        out[..4].copy_from_slice(&SERVICE_ABI_MAGIC);
        out[4..6].copy_from_slice(&SERVICE_ABI_VERSION.to_le_bytes());
        out[6..8].copy_from_slice(&(self.op as u16).to_le_bytes());
        out[8] = self.service_id;
        out[9] = self.flags;
        out[10..14].copy_from_slice(&self.request_id.to_le_bytes());
        out[14..16].copy_from_slice(&self.payload_len.to_le_bytes());
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < SERVICE_ABI_HEADER_SIZE || buf[..4] != SERVICE_ABI_MAGIC {
            return None;
        }
        if u16::from_le_bytes([buf[4], buf[5]]) != SERVICE_ABI_VERSION {
            return None;
        }
        let op = ServiceOp::from_u16(u16::from_le_bytes([buf[6], buf[7]]))?;
        let service_id = buf[8];
        if !(SERVICE_ID_MIN..=SERVICE_ID_MAX).contains(&service_id) {
            return None;
        }
        let payload_len = u16::from_le_bytes([buf[14], buf[15]]);
        if payload_len > SERVICE_ABI_MAX_PAYLOAD {
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

/// RAPP capability IDs allowed for a Ring-3 system service before a dedicated
/// hardware ABI exists. Privileged process/storage/network/control powers are
/// intentionally excluded.
pub fn service_capability_allowed(service_id: u8, capability: u16) -> bool {
    match service_id {
        1 => matches!(capability, 1 | 2 | 3 | 6 | 17),
        2 => matches!(capability, 1 | 3 | 6 | 17),
        3 => matches!(capability, 1 | 3 | 6 | 9 | 10 | 17),
        4 => matches!(capability, 1 | 3 | 6 | 12 | 17),
        5 => matches!(capability, 1 | 3 | 6 | 17 | 19),
        6 => matches!(capability, 1 | 3 | 6 | 17 | 20),
        7 => matches!(capability, 1 | 3 | 6 | 17 | 21),
        8 => matches!(capability, 1 | 3 | 6 | 17 | 22),
        9 => matches!(capability, 1 | 2 | 3 | 6 | 9 | 17),
        10 => matches!(capability, 1 | 2 | 3 | 5 | 6 | 9 | 17),
        11 => matches!(capability, 1 | 3 | 6 | 17 | 23),
        12 => matches!(capability, 1 | 3 | 6 | 9 | 10 | 17),
        13 => matches!(capability, 1 | 3 | 6 | 9 | 10 | 17),
        14 => matches!(capability, 1 | 3 | 6 | 9 | 10 | 17),
        15 => matches!(capability, 1 | 3 | 6 | 9 | 10 | 17),
        _ => false,
    }
}

pub fn service_id_from_package_id(package_id: &str) -> Option<u8> {
    match package_id {
        "org.interos.service.displayd" => Some(1),
        "org.interos.service.inputd" => Some(2),
        "org.interos.service.storaged" => Some(3),
        "org.interos.service.netd" => Some(4),
        "org.interos.service.powerd" => Some(5),
        "org.interos.service.modemd" => Some(6),
        "org.interos.service.audiod" => Some(7),
        "org.interos.service.camerad" => Some(8),
        "org.interos.compositor" | "org.interos.service.compositor" => Some(9),
        "org.interos.launcher" | "org.interos.service.launcher" => Some(10),
        "org.interos.service.sensord" => Some(11),
        "org.interos.service.aid" => Some(12),
        "org.interos.service.searchd" => Some(13),
        "org.interos.service.agentd" => Some(14),
        "org.interos.service.voiced" => Some(15),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_abi_encodes_and_decodes_lowest_valid_id() {
        let msg = ServiceMessage {
            op: ServiceOp::Register,
            service_id: SERVICE_ID_MIN,
            flags: 0x42,
            request_id: 100,
            payload_len: 64,
        };
        let mut buf = [0u8; SERVICE_ABI_HEADER_SIZE];
        msg.encode(&mut buf);
        let decoded = ServiceMessage::decode(&buf).expect("lowest valid ID must decode");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn service_abi_encodes_and_decodes_highest_valid_id() {
        let msg = ServiceMessage {
            op: ServiceOp::Shutdown,
            service_id: SERVICE_ID_MAX,
            flags: 0,
            request_id: 200,
            payload_len: 0,
        };
        let mut buf = [0u8; SERVICE_ABI_HEADER_SIZE];
        msg.encode(&mut buf);
        let decoded = ServiceMessage::decode(&buf).expect("highest valid ID must decode");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn service_abi_supports_inference_service_id_12() {
        let msg = ServiceMessage {
            op: ServiceOp::Heartbeat,
            service_id: 12, // aid
            flags: 0x01,
            request_id: 300,
            payload_len: 128,
        };
        let mut buf = [0u8; SERVICE_ABI_HEADER_SIZE];
        msg.encode(&mut buf);
        let decoded = ServiceMessage::decode(&buf).expect("inference service (ID 12) must decode");
        assert_eq!(decoded, msg);
        assert_eq!(
            service_id_from_package_id("org.interos.service.aid"),
            Some(12)
        );
    }

    #[test]
    fn service_abi_rejects_zero_and_reserved_service_ids() {
        let mut buf = [0u8; SERVICE_ABI_HEADER_SIZE];
        let msg = ServiceMessage {
            op: ServiceOp::Query,
            service_id: 1,
            flags: 0,
            request_id: 1,
            payload_len: 0,
        };
        msg.encode(&mut buf);

        // ID 0 (Init supervisor) is not a wire service endpoint
        buf[8] = 0;
        assert_eq!(ServiceMessage::decode(&buf), None);

        // ID 16 is beyond SERVICE_ID_MAX
        buf[8] = 16;
        assert_eq!(ServiceMessage::decode(&buf), None);

        // ID 255 is invalid
        buf[8] = 255;
        assert_eq!(ServiceMessage::decode(&buf), None);
    }

    #[test]
    fn service_abi_rejects_malformed_messages() {
        let mut buf = [0u8; SERVICE_ABI_HEADER_SIZE];
        let msg = ServiceMessage {
            op: ServiceOp::Heartbeat,
            service_id: 1,
            flags: 0,
            request_id: 1,
            payload_len: 0,
        };
        msg.encode(&mut buf);

        // Buffer shorter than header size
        assert_eq!(ServiceMessage::decode(&buf[..15]), None);

        // Bad magic
        let mut bad_magic = buf;
        bad_magic[0] = b'X';
        assert_eq!(ServiceMessage::decode(&bad_magic), None);

        // Bad version
        let mut bad_version = buf;
        bad_version[4] = 99;
        assert_eq!(ServiceMessage::decode(&bad_version), None);

        // Unknown opcode
        let mut bad_op = buf;
        bad_op[6] = 0xFF;
        assert_eq!(ServiceMessage::decode(&bad_op), None);

        // Oversized payload (> 4096)
        let mut bad_payload = buf;
        bad_payload[14..16].copy_from_slice(&4097u16.to_le_bytes());
        assert_eq!(ServiceMessage::decode(&bad_payload), None);
    }
}
