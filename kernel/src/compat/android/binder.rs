// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android Binder IPC Transaction Mediator (Zone 4 Isolation)
//!
//! Mediates Android Binder RPC transactions between Android container processes
//! and host microkernel services, enforcing zero-trust capability validation.
//! Android processes are strictly barred from ambient IPC.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::task::TaskId;

/// Capability object namespace for Android Binder services.
/// A caller must hold IpcSend against the exact target object; a generic Binder
/// capability must never authorize arbitrary service handles.
pub const BINDER_SERVICE_OBJECT_BASE: u64 = 0x4249_4E44_0000_0000;

pub const fn binder_service_object(handle: u32) -> u64 {
    BINDER_SERVICE_OBJECT_BASE | handle as u64
}

/// Sovereign native microG / GMS Compatibility Binder Handles.
pub const HANDLE_MICROG_GMS_BROKER: u32 = 0x1001;
pub const HANDLE_MICROG_LOCATION: u32 = 0x1002;
pub const HANDLE_MICROG_GCM_PUSH: u32 = 0x1003;
pub const HANDLE_MICROG_ADS_ID: u32 = 0x1004;

/// Android Binder transaction command codes written to `/dev/binder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BinderCommand {
    BcTransaction = 0x40046300,
    BcReply = 0x40046301,
    BcAcquire = 0x40046302,
    BcRelease = 0x40046303,
}

/// Android Binder return protocol response codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BinderReturn {
    BrTransaction = 0x80047200,
    BrReply = 0x80047201,
    BrTransactionComplete = 0x7202,
    BrFailedReply = 0x7203,
    BrDeadReply = 0x7204,
    BrSecurityDenied = 0x7205,
}

/// Android Binder transaction payload data.
#[derive(Debug, Clone)]
pub struct BinderTransaction {
    /// Target service handle (0 = ServiceManager).
    pub target_handle: u32,
    /// Method / opcode code within interface.
    pub code: u32,
    /// Async or sync flags.
    pub flags: u32,
    /// Raw serialized payload buffer.
    pub data: Vec<u8>,
}

/// Security audit log entry for unauthorized Android Binder access attempts.
#[derive(Debug, Clone)]
pub struct BinderSecurityAudit {
    pub caller_task: TaskId,
    pub target_handle: u32,
    pub code: u32,
    pub reason: &'static str,
}

/// Global mediator for Android Binder IPC communication.
pub struct BinderMediator {
    registered_services: BTreeMap<String, u32>,
    audit_log: Vec<BinderSecurityAudit>,
}

impl BinderMediator {
    /// Creates a new Binder mediator.
    pub const fn new() -> Self {
        Self {
            registered_services: BTreeMap::new(),
            audit_log: Vec::new(),
        }
    }

    /// Registers a virtual Android service handle (e.g. `activity`, `package`).
    pub fn register_service(&mut self, name: &str, handle: u32) {
        self.registered_services.insert(String::from(name), handle);
    }

    /// Mediates and capability-verifies a Binder transaction from an Android task.
    ///
    /// Zone 4 Security Boundary:
    /// - Service handle 0 (ServiceManager) queries are mediated.
    /// - Calling protected native services requires the Android task to present an explicit
    ///   `IpcSend` capability token specifically bound to that service.
    /// - Unauthorized access attempts are rejected with `BinderReturn::BrSecurityDenied`.
    pub fn dispatch_transaction(
        &mut self,
        caller: TaskId,
        caller_caps: &CapabilityTable,
        _cap_slot: usize,
        transaction: &BinderTransaction,
    ) -> Result<BinderReturn, BinderReturn> {
        // ServiceManager query (Handle 0, Code 1 = getService)
        if transaction.data.len() > 64 * 1024 {
            self.audit_log.push(BinderSecurityAudit {
                caller_task: caller,
                target_handle: transaction.target_handle,
                code: transaction.code,
                reason: "Android Binder payload exceeds bounded transaction size",
            });
            return Err(BinderReturn::BrSecurityDenied);
        }
        if transaction.target_handle == 0 {
            // ServiceManager enumeration is intentionally read-only and carries no
            // arbitrary transaction payload or native capability.
            if transaction.code != 1 || !transaction.data.is_empty() {
                return Err(BinderReturn::BrSecurityDenied);
            }
            return Ok(BinderReturn::BrTransactionComplete);
        }

        // Bounded Service Call: Verify capability token
        if caller_caps.authorize_object(CapabilityType::IpcSend, Rights::WRITE, binder_service_object(transaction.target_handle)) == false {
            self.audit_log.push(BinderSecurityAudit {
                caller_task: caller,
                target_handle: transaction.target_handle,
                code: transaction.code,
                reason: "Android Zone 4 caller missing required IpcSend capability",
            });
            return Err(BinderReturn::BrSecurityDenied);
        }

        Ok(BinderReturn::BrReply)
    }

    /// Mediates a microG GMS service request into a sovereign, privacy-respecting response.
    pub fn handle_microg_transaction(
        &mut self,
        target_handle: u32,
        _code: u32,
        _data: &[u8],
        reply: &mut [u8],
    ) -> Result<usize, BinderReturn> {
        match target_handle {
            HANDLE_MICROG_LOCATION => {
                let mut writer = ParcelWriter::new(reply);
                writer.write_no_exception().map_err(|_| BinderReturn::BrFailedReply)?;
                writer.write_i32(0).map_err(|_| BinderReturn::BrFailedReply)?; // has_location = false (honest no satellite fix)
                Ok(writer.len())
            }
            HANDLE_MICROG_ADS_ID => {
                let mut writer = ParcelWriter::new(reply);
                writer.write_no_exception().map_err(|_| BinderReturn::BrFailedReply)?;
                writer.write_string16_utf8("00000000-0000-0000-0000-000000000000")
                    .map_err(|_| BinderReturn::BrFailedReply)?;
                writer.write_i32(1).map_err(|_| BinderReturn::BrFailedReply)?; // limit_ad_tracking = 1
                Ok(writer.len())
            }
            HANDLE_MICROG_GMS_BROKER => {
                let mut writer = ParcelWriter::new(reply);
                writer.write_no_exception().map_err(|_| BinderReturn::BrFailedReply)?;
                writer.write_i32(0).map_err(|_| BinderReturn::BrFailedReply)?;
                Ok(writer.len())
            }
            HANDLE_MICROG_GCM_PUSH => {
                let mut writer = ParcelWriter::new(reply);
                writer.write_no_exception().map_err(|_| BinderReturn::BrFailedReply)?;
                writer.write_string16_utf8("sovereign-interos-local-token")
                    .map_err(|_| BinderReturn::BrFailedReply)?;
                Ok(writer.len())
            }
            _ => Err(BinderReturn::BrSecurityDenied),
        }
    }

    /// Returns the total number of security violations intercepted and blocked.
    pub fn violation_count(&self) -> usize {
        self.audit_log.len()
    }
}

/// Zero-allocation bounded AIDL Parcel reader for microG and Android Binder transactions.
pub struct ParcelReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ParcelReader<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn read_i32(&mut self) -> Option<i32> {
        if self.offset + 4 > self.data.len() {
            return None;
        }
        let val = i32::from_le_bytes([
            self.data[self.offset],
            self.data[self.offset + 1],
            self.data[self.offset + 2],
            self.data[self.offset + 3],
        ]);
        self.offset += 4;
        Some(val)
    }

    pub fn read_u32(&mut self) -> Option<u32> {
        self.read_i32().map(|v| v as u32)
    }

    pub fn read_i64(&mut self) -> Option<i64> {
        if self.offset + 8 > self.data.len() {
            return None;
        }
        let bytes: [u8; 8] = [
            self.data[self.offset],
            self.data[self.offset + 1],
            self.data[self.offset + 2],
            self.data[self.offset + 3],
            self.data[self.offset + 4],
            self.data[self.offset + 5],
            self.data[self.offset + 6],
            self.data[self.offset + 7],
        ];
        let val = i64::from_le_bytes(bytes);
        self.offset += 8;
        Some(val)
    }

    pub fn read_interface_token(&mut self) -> Option<&'a [u8]> {
        let _strict_mode = self.read_i32()?;
        let char_count = self.read_i32()? as usize;
        let byte_len = char_count.checked_mul(2)?;
        let padded_len = (byte_len + 3) & !3;
        if self.offset + padded_len > self.data.len() {
            return None;
        }
        let token_bytes = &self.data[self.offset..self.offset + byte_len];
        self.offset += padded_len;
        Some(token_bytes)
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }
}

/// Zero-allocation bounded AIDL Parcel writer for microG responses.
pub struct ParcelWriter<'a> {
    buffer: &'a mut [u8],
    offset: usize,
}

impl<'a> ParcelWriter<'a> {
    pub fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, offset: 0 }
    }

    pub fn write_i32(&mut self, val: i32) -> Result<(), ()> {
        if self.offset + 4 > self.buffer.len() {
            return Err(());
        }
        self.buffer[self.offset..self.offset + 4].copy_from_slice(&val.to_le_bytes());
        self.offset += 4;
        Ok(())
    }

    pub fn write_u32(&mut self, val: u32) -> Result<(), ()> {
        self.write_i32(val as i32)
    }

    pub fn write_i64(&mut self, val: i64) -> Result<(), ()> {
        if self.offset + 8 > self.buffer.len() {
            return Err(());
        }
        self.buffer[self.offset..self.offset + 8].copy_from_slice(&val.to_le_bytes());
        self.offset += 8;
        Ok(())
    }

    pub fn write_no_exception(&mut self) -> Result<(), ()> {
        self.write_i32(0) // EX_NONE = 0
    }

    pub fn write_string16_utf8(&mut self, text: &str) -> Result<(), ()> {
        let char_count = text.chars().count();
        self.write_i32(char_count as i32)?;
        let byte_len = char_count * 2;
        let padded_len = (byte_len + 3) & !3;
        if self.offset + padded_len > self.buffer.len() {
            return Err(());
        }
        for (i, c) in text.encode_utf16().enumerate() {
            let start = self.offset + i * 2;
            self.buffer[start..start + 2].copy_from_slice(&c.to_le_bytes());
        }
        for i in byte_len..padded_len {
            self.buffer[self.offset + i] = 0;
        }
        self.offset += padded_len;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.offset
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buffer[..self.offset]
    }
}

/// Global system-wide Android Binder mediator.
pub static BINDER_MEDIATOR: Mutex<BinderMediator> = Mutex::new(BinderMediator::new());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    #[test]
    fn test_service_manager_enumeration_contract() {
        let mut mediator = BinderMediator::new();
        let empty_caps = CapabilityTable::new(4);

        // Valid ServiceManager getService query (handle 0, code 1, empty data)
        let valid_sm_tx = BinderTransaction {
            target_handle: 0,
            code: 1,
            flags: 0,
            data: Vec::new(),
        };
        assert_eq!(
            mediator.dispatch_transaction(TaskId(10), &empty_caps, 0, &valid_sm_tx),
            Ok(BinderReturn::BrTransactionComplete)
        );

        // Disallowed ServiceManager query (code != 1)
        let bad_code_tx = BinderTransaction {
            target_handle: 0,
            code: 2,
            flags: 0,
            data: Vec::new(),
        };
        assert_eq!(
            mediator.dispatch_transaction(TaskId(10), &empty_caps, 0, &bad_code_tx),
            Err(BinderReturn::BrSecurityDenied)
        );

        // Disallowed ServiceManager query with payload
        let bad_payload_tx = BinderTransaction {
            target_handle: 0,
            code: 1,
            flags: 0,
            data: alloc::vec![1, 2, 3],
        };
        assert_eq!(
            mediator.dispatch_transaction(TaskId(10), &empty_caps, 0, &bad_payload_tx),
            Err(BinderReturn::BrSecurityDenied)
        );
    }

    #[test]
    fn test_microg_handles_capability_gating() {
        let mut mediator = BinderMediator::new();
        let empty_caps = CapabilityTable::new(4);

        let microg_handles = [
            HANDLE_MICROG_GMS_BROKER,
            HANDLE_MICROG_LOCATION,
            HANDLE_MICROG_GCM_PUSH,
            HANDLE_MICROG_ADS_ID,
        ];

        // 1. Without capability, all microG handles fail closed
        for &handle in &microg_handles {
            let tx = BinderTransaction {
                target_handle: handle,
                code: 1,
                flags: 0,
                data: Vec::new(),
            };
            assert_eq!(
                mediator.dispatch_transaction(TaskId(20), &empty_caps, 0, &tx),
                Err(BinderReturn::BrSecurityDenied)
            );
        }
        assert_eq!(mediator.violation_count(), 4);

        // 2. Grant capability only for HANDLE_MICROG_LOCATION
        let mut location_caps = CapabilityTable::new(4);
        location_caps.insert(Capability {
            id: crate::capability::CapabilityId(1),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: binder_service_object(HANDLE_MICROG_LOCATION),
        }).unwrap();

        let location_tx = BinderTransaction {
            target_handle: HANDLE_MICROG_LOCATION,
            code: 1,
            flags: 0,
            data: Vec::new(),
        };
        assert_eq!(
            mediator.dispatch_transaction(TaskId(21), &location_caps, 0, &location_tx),
            Ok(BinderReturn::BrReply)
        );

        // 3. HANDLE_MICROG_LOCATION capability must NOT authorize HANDLE_MICROG_GCM_PUSH
        let push_tx = BinderTransaction {
            target_handle: HANDLE_MICROG_GCM_PUSH,
            code: 1,
            flags: 0,
            data: Vec::new(),
        };
        assert_eq!(
            mediator.dispatch_transaction(TaskId(21), &location_caps, 0, &push_tx),
            Err(BinderReturn::BrSecurityDenied)
        );
    }

    #[test]
    fn test_oversized_transaction_rejected() {
        let mut mediator = BinderMediator::new();
        let empty_caps = CapabilityTable::new(4);

        let oversized_tx = BinderTransaction {
            target_handle: 0,
            code: 1,
            flags: 0,
            data: alloc::vec![0u8; 64 * 1024 + 1],
        };
        assert_eq!(
            mediator.dispatch_transaction(TaskId(30), &empty_caps, 0, &oversized_tx),
            Err(BinderReturn::BrSecurityDenied)
        );
    }

    #[test]
    fn test_parcel_reader_writer_roundtrip() {
        let mut buf = [0u8; 128];
        let mut writer = ParcelWriter::new(&mut buf);
        writer.write_no_exception().unwrap();
        writer.write_i32(42).unwrap();
        writer.write_u32(100).unwrap();
        writer.write_i64(0x0102030405060708).unwrap();
        writer.write_string16_utf8("InterOS").unwrap();
        let written_len = writer.len();

        let mut reader = ParcelReader::new(&buf[..written_len]);
        assert_eq!(reader.read_i32(), Some(0)); // no exception
        assert_eq!(reader.read_i32(), Some(42));
        assert_eq!(reader.read_u32(), Some(100));
        assert_eq!(reader.read_i64(), Some(0x0102030405060708));
    }

    #[test]
    fn test_handle_microg_transaction() {
        let mut mediator = BinderMediator::new();
        let mut reply = [0u8; 256];

        // 1. Ads ID returns zeroed UUID + limit tracking = 1
        let len = mediator.handle_microg_transaction(HANDLE_MICROG_ADS_ID, 1, &[], &mut reply).unwrap();
        let mut reader = ParcelReader::new(&reply[..len]);
        assert_eq!(reader.read_i32(), Some(0)); // no exception
        let char_count = reader.read_i32().unwrap();
        assert_eq!(char_count, 36); // length of UUID string

        // 2. Location returns honest no satellite fix
        let len = mediator.handle_microg_transaction(HANDLE_MICROG_LOCATION, 1, &[], &mut reply).unwrap();
        let mut reader = ParcelReader::new(&reply[..len]);
        assert_eq!(reader.read_i32(), Some(0)); // no exception
        assert_eq!(reader.read_i32(), Some(0)); // has_location = false
    }
}
