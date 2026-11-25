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

use interos_android_format::manifest::{CapabilityTable, CapabilityType, Rights};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TaskId(pub u64);

/// Capability object namespace for Android Binder services.
/// A caller must hold IpcSend against the exact target object; a generic Binder
/// capability must never authorize arbitrary service handles.
pub const BINDER_SERVICE_OBJECT_BASE: u64 = 0x4249_4E44_0000_0000;

pub const fn binder_service_object(handle: u32) -> u64 {
    BINDER_SERVICE_OBJECT_BASE | handle as u64
}

pub const MAX_PARCEL_CAPACITY: usize = 64 * 1024;
pub const MAX_STRING_LENGTH: usize = 32 * 1024;
pub const MAX_ARRAY_LENGTH: usize = 64 * 1024;

// Standard IBinder transaction codes
pub const FIRST_CALL_TRANSACTION: u32 = 0x0000_0001;
pub const LAST_CALL_TRANSACTION: u32 = 0x00FF_FFFF;
pub const PING_TRANSACTION: u32 = 0x5F50_4E47;          // "_PNG"
pub const DUMP_TRANSACTION: u32 = 0x5F44_4D50;          // "_DMP"
pub const SHELL_COMMAND_TRANSACTION: u32 = 0x5F43_4D44; // "_CMD"
pub const INTERFACE_TRANSACTION: u32 = 0x5F4E_5446;     // "_NTF"
pub const SYSPROPS_TRANSACTION: u32 = 0x5F53_5052;      // "_SPR"

// Binder transaction flags
pub const FLAG_ONEWAY: u32 = 0x0000_0001;
pub const FLAG_CLEAR_BUF: u32 = 0x0000_0020;

// AIDL Exception codes
pub const EX_NONE: i32 = 0;
pub const EX_SECURITY: i32 = -1;
pub const EX_BAD_PARCELABLE: i32 = -2;
pub const EX_ILLEGAL_ARGUMENT: i32 = -3;
pub const EX_NULL_POINTER: i32 = -4;
pub const EX_ILLEGAL_STATE: i32 = -5;
pub const EX_NETWORK_MAIN_THREAD: i32 = -6;
pub const EX_UNSUPPORTED_OPERATION: i32 = -7;
pub const EX_SERVICE_SPECIFIC: i32 = -8;
pub const EX_PARCELABLE: i32 = -9;
pub const EX_TRANSACTION_FAILED: i32 = -129;

pub const STRICT_MODE_PENALTY_GATHER: i32 = 0x0040_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParcelError {
    BufferOverRead,
    BufferOverflow,
    MalformedNegativeLength(i32),
    UnboundedLength(usize),
    ArithmeticOverflow,
    InvalidUtf8,
    InvalidUtf16,
    MissingNullTerminator,
    MalformedPadding,
    InterfaceMismatch,
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

/// Heap-allocated, serialization-safe Android Parcel implementation for IPC and framework usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parcel {
    pub data: Vec<u8>,
    pub position: usize,
}

impl Parcel {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            position: 0,
        }
    }

    pub fn obtain() -> Self {
        Self::new()
    }

    pub fn recycle(&mut self) {
        self.data.clear();
        self.position = 0;
    }

    pub fn data_size(&self) -> usize {
        self.data.len()
    }

    pub fn data_position(&self) -> usize {
        self.position
    }

    pub fn set_data_position(&mut self, pos: usize) {
        self.position = pos.min(self.data.len());
    }

    pub fn write_int(&mut self, val: i32) {
        self.data.extend_from_slice(&val.to_le_bytes());
    }

    pub fn read_int(&mut self) -> Option<i32> {
        if self.position + 4 <= self.data.len() {
            let val = i32::from_le_bytes(self.data[self.position..self.position + 4].try_into().ok()?);
            self.position += 4;
            Some(val)
        } else {
            None
        }
    }

    pub fn write_long(&mut self, val: i64) {
        self.data.extend_from_slice(&val.to_le_bytes());
    }

    pub fn read_long(&mut self) -> Option<i64> {
        if self.position + 8 <= self.data.len() {
            let val = i64::from_le_bytes(self.data[self.position..self.position + 8].try_into().ok()?);
            self.position += 8;
            Some(val)
        } else {
            None
        }
    }

    pub fn write_float(&mut self, val: f32) {
        self.data.extend_from_slice(&val.to_bits().to_le_bytes());
    }

    pub fn read_float(&mut self) -> Option<f32> {
        if self.position + 4 <= self.data.len() {
            let val = u32::from_le_bytes(self.data[self.position..self.position + 4].try_into().ok()?);
            self.position += 4;
            Some(f32::from_bits(val))
        } else {
            None
        }
    }

    pub fn write_double(&mut self, val: f64) {
        self.data.extend_from_slice(&val.to_bits().to_le_bytes());
    }

    pub fn read_double(&mut self) -> Option<f64> {
        if self.position + 8 <= self.data.len() {
            let val = u64::from_le_bytes(self.data[self.position..self.position + 8].try_into().ok()?);
            self.position += 8;
            Some(f64::from_bits(val))
        } else {
            None
        }
    }

    pub fn write_string(&mut self, val: &str) {
        let bytes = val.as_bytes();
        let len = bytes.len().min(MAX_STRING_LENGTH);
        self.write_int(len as i32);
        self.data.extend_from_slice(&bytes[..len]);
        let pad = (4 - (len % 4)) % 4;
        for _ in 0..pad {
            self.data.push(0);
        }
    }

    pub fn read_string(&mut self) -> Option<String> {
        let raw_len = self.read_int()?;
        if raw_len == -1 {
            return None; // null string representation
        }
        if raw_len < 0 {
            return None; // malformed negative length rejected
        }
        let len = raw_len as usize;
        if len > MAX_STRING_LENGTH || len > self.data.len().saturating_sub(self.position) {
            return None; // bounded allocation enforcement
        }
        let end = self.position.checked_add(len)?;
        let str_bytes = self.data.get(self.position..end)?;
        let s = String::from(core::str::from_utf8(str_bytes).ok()?);
        self.position = end;
        let pad = (4 - (len % 4)) % 4;
        self.position = self.position.checked_add(pad)?.min(self.data.len());
        Some(s)
    }

    pub fn write_strong_binder(&mut self, handle: u32) {
        self.data.extend_from_slice(&handle.to_le_bytes());
    }

    pub fn read_strong_binder(&mut self) -> Option<u32> {
        if self.position + 4 <= self.data.len() {
            let h = u32::from_le_bytes(self.data[self.position..self.position + 4].try_into().ok()?);
            self.position += 4;
            Some(h)
        } else {
            None
        }
    }

    pub fn write_byte_array(&mut self, slice: &[u8]) {
        let len = slice.len().min(MAX_ARRAY_LENGTH);
        self.write_int(len as i32);
        self.data.extend_from_slice(&slice[..len]);
        let pad = (4 - (len % 4)) % 4;
        for _ in 0..pad {
            self.data.push(0);
        }
    }

    pub fn create_byte_array(&mut self) -> Option<Vec<u8>> {
        let raw_len = self.read_int()?;
        if raw_len == -1 {
            return None; // null array representation
        }
        if raw_len < 0 {
            return None; // malformed negative length rejected
        }
        let len = raw_len as usize;
        if len > MAX_ARRAY_LENGTH || len > self.data.len().saturating_sub(self.position) {
            return None; // bounded allocation enforcement
        }
        let end = self.position.checked_add(len)?;
        let bytes = self.data.get(self.position..end)?.to_vec();
        self.position = end;
        let pad = (4 - (len % 4)) % 4;
        self.position = self.position.checked_add(pad)?.min(self.data.len());
        Some(bytes)
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

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
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

    pub fn read_float(&mut self) -> Option<f32> {
        self.read_u32().map(f32::from_bits)
    }

    pub fn read_double(&mut self) -> Option<f64> {
        self.read_i64().map(|v| f64::from_bits(v as u64))
    }

    /// Reads an AOSP UTF-8 string (length + bytes + '\0' + padding).
    pub fn read_string8(&mut self) -> Result<Option<&'a str>, ParcelError> {
        let raw_len = self.read_i32().ok_or(ParcelError::BufferOverRead)?;
        if raw_len == -1 {
            return Ok(None);
        }
        if raw_len < -1 {
            return Err(ParcelError::MalformedNegativeLength(raw_len));
        }
        let byte_len = raw_len as usize;
        if byte_len > MAX_STRING_LENGTH || byte_len > self.remaining() {
            return Err(ParcelError::UnboundedLength(byte_len));
        }
        let with_null = byte_len.checked_add(1).ok_or(ParcelError::ArithmeticOverflow)?;
        let pad = (4 - (with_null % 4)) % 4;
        let total_wire_len = with_null.checked_add(pad).ok_or(ParcelError::ArithmeticOverflow)?;
        let end = self.offset.checked_add(total_wire_len).ok_or(ParcelError::ArithmeticOverflow)?;
        if end > self.data.len() {
            return Err(ParcelError::BufferOverRead);
        }

        let str_bytes = &self.data[self.offset..self.offset + byte_len];
        if self.data[self.offset + byte_len] != 0 {
            return Err(ParcelError::MissingNullTerminator);
        }
        let s = core::str::from_utf8(str_bytes).map_err(|_| ParcelError::InvalidUtf8)?;
        self.offset = end;
        Ok(Some(s))
    }

    /// Reads an AOSP UTF-16 string (length + code units + 0x0000 + padding).
    pub fn read_string16(&mut self) -> Result<Option<String>, ParcelError> {
        let raw_len = self.read_i32().ok_or(ParcelError::BufferOverRead)?;
        if raw_len == -1 {
            return Ok(None);
        }
        if raw_len < -1 {
            return Err(ParcelError::MalformedNegativeLength(raw_len));
        }
        let char_count = raw_len as usize;
        if char_count > MAX_STRING_LENGTH {
            return Err(ParcelError::UnboundedLength(char_count));
        }
        let byte_len = char_count.checked_mul(2).ok_or(ParcelError::ArithmeticOverflow)?;
        let with_null = byte_len.checked_add(2).ok_or(ParcelError::ArithmeticOverflow)?;
        let pad = (4 - (with_null % 4)) % 4;
        let total_wire_len = with_null.checked_add(pad).ok_or(ParcelError::ArithmeticOverflow)?;
        let end = self.offset.checked_add(total_wire_len).ok_or(ParcelError::ArithmeticOverflow)?;
        if end > self.data.len() {
            return Err(ParcelError::BufferOverRead);
        }

        // Verify 16-bit null terminator
        let term_start = self.offset + byte_len;
        let term_bytes = [self.data[term_start], self.data[term_start + 1]];
        if u16::from_le_bytes(term_bytes) != 0 {
            return Err(ParcelError::MissingNullTerminator);
        }

        // Decode UTF-16 LE code units
        let u16_iter = (0..char_count).map(|i| {
            let start = self.offset + i * 2;
            u16::from_le_bytes([self.data[start], self.data[start + 1]])
        });
        let s = char::decode_utf16(u16_iter)
            .collect::<Result<String, _>>()
            .map_err(|_| ParcelError::InvalidUtf16)?;

        self.offset = end;
        Ok(Some(s))
    }

    /// Validates and enforces an AIDL interface token against an expected descriptor.
    pub fn enforce_interface(&mut self, expected_descriptor: &str) -> Result<(), ParcelError> {
        let _strict_mode = self.read_i32().ok_or(ParcelError::BufferOverRead)?;
        let descriptor = self.read_string16()?.ok_or(ParcelError::InterfaceMismatch)?;
        if descriptor != expected_descriptor {
            return Err(ParcelError::InterfaceMismatch);
        }
        Ok(())
    }

    pub fn read_byte_array(&mut self) -> Result<Option<&'a [u8]>, ParcelError> {
        let raw_len = self.read_i32().ok_or(ParcelError::BufferOverRead)?;
        if raw_len == -1 {
            return Ok(None);
        }
        if raw_len < -1 {
            return Err(ParcelError::MalformedNegativeLength(raw_len));
        }
        let len = raw_len as usize;
        if len > MAX_ARRAY_LENGTH || len > self.remaining() {
            return Err(ParcelError::UnboundedLength(len));
        }
        let pad = (4 - (len % 4)) % 4;
        let total_wire_len = len.checked_add(pad).ok_or(ParcelError::ArithmeticOverflow)?;
        let end = self.offset.checked_add(total_wire_len).ok_or(ParcelError::ArithmeticOverflow)?;
        if end > self.data.len() {
            return Err(ParcelError::BufferOverRead);
        }
        let slice = &self.data[self.offset..self.offset + len];
        self.offset = end;
        Ok(Some(slice))
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

    pub fn write_float(&mut self, val: f32) -> Result<(), ()> {
        self.write_u32(val.to_bits())
    }

    pub fn write_double(&mut self, val: f64) -> Result<(), ()> {
        self.write_i64(val.to_bits() as i64)
    }

    pub fn write_no_exception(&mut self) -> Result<(), ()> {
        self.write_i32(EX_NONE)
    }

    pub fn write_exception(&mut self, code: i32, msg: &str) -> Result<(), ()> {
        self.write_i32(code)?;
        self.write_string16_utf8(msg)
    }

    pub fn write_string16_utf8(&mut self, text: &str) -> Result<(), ()> {
        let utf16_units = text.encode_utf16().count();
        if utf16_units > MAX_STRING_LENGTH {
            return Err(());
        }
        self.write_i32(utf16_units as i32)?;

        let byte_len = utf16_units.checked_mul(2).ok_or(())?;
        let with_null = byte_len.checked_add(2).ok_or(())?;
        let pad = (4 - (with_null % 4)) % 4;
        let total_wire_len = with_null.checked_add(pad).ok_or(())?;
        let end = self.offset.checked_add(total_wire_len).ok_or(())?;
        if end > self.buffer.len() {
            return Err(());
        }

        // Write UTF-16 LE code units
        for (i, unit) in text.encode_utf16().enumerate() {
            let start = self.offset + i * 2;
            self.buffer[start..start + 2].copy_from_slice(&unit.to_le_bytes());
        }
        // Write 16-bit null terminator (0x0000)
        self.buffer[self.offset + byte_len..self.offset + byte_len + 2].copy_from_slice(&[0, 0]);
        // Zero-fill padding
        for i in with_null..total_wire_len {
            self.buffer[self.offset + i] = 0;
        }

        self.offset = end;
        Ok(())
    }

    pub fn write_string8_utf8(&mut self, text: &str) -> Result<(), ()> {
        let byte_len = text.len();
        if byte_len > MAX_STRING_LENGTH {
            return Err(());
        }
        self.write_i32(byte_len as i32)?;

        let with_null = byte_len.checked_add(1).ok_or(())?;
        let pad = (4 - (with_null % 4)) % 4;
        let total_wire_len = with_null.checked_add(pad).ok_or(())?;
        let end = self.offset.checked_add(total_wire_len).ok_or(())?;
        if end > self.buffer.len() {
            return Err(());
        }

        self.buffer[self.offset..self.offset + byte_len].copy_from_slice(text.as_bytes());
        self.buffer[self.offset + byte_len] = 0; // null terminator
        for i in with_null..total_wire_len {
            self.buffer[self.offset + i] = 0;
        }

        self.offset = end;
        Ok(())
    }

    pub fn write_interface_token(&mut self, descriptor: &str) -> Result<(), ()> {
        self.write_i32(STRICT_MODE_PENALTY_GATHER)?;
        self.write_string16_utf8(descriptor)
    }

    pub fn write_byte_array(&mut self, slice: &[u8]) -> Result<(), ()> {
        let len = slice.len();
        if len > MAX_ARRAY_LENGTH {
            return Err(());
        }
        self.write_i32(len as i32)?;
        let pad = (4 - (len % 4)) % 4;
        let total_wire_len = len.checked_add(pad).ok_or(())?;
        let end = self.offset.checked_add(total_wire_len).ok_or(())?;
        if end > self.buffer.len() {
            return Err(());
        }
        self.buffer[self.offset..self.offset + len].copy_from_slice(slice);
        for i in len..total_wire_len {
            self.buffer[self.offset + i] = 0;
        }
        self.offset = end;
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
    use interos_android_format::manifest::{Capability, CapabilityId};

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
            id: CapabilityId(1),
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

    #[test]
    fn test_negative_length_handling() {
        // 1. Valid null string (-1)
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&(-1i32).to_le_bytes());
        let mut reader = ParcelReader::new(&buf);
        assert_eq!(reader.read_string8(), Ok(None));
        assert_eq!(reader.offset(), 4);

        // 2. Malformed negative length (-2) must fail closed
        let mut bad_buf = [0u8; 16];
        bad_buf[0..4].copy_from_slice(&(-2i32).to_le_bytes());
        let mut bad_reader = ParcelReader::new(&bad_buf);
        assert_eq!(bad_reader.read_string8(), Err(ParcelError::MalformedNegativeLength(-2)));

        // 3. Minimum i32 (-2147483648) must fail closed without underflow/panic
        let mut min_buf = [0u8; 16];
        min_buf[0..4].copy_from_slice(&i32::MIN.to_le_bytes());
        let mut min_reader = ParcelReader::new(&min_buf);
        assert_eq!(min_reader.read_string16(), Err(ParcelError::MalformedNegativeLength(i32::MIN)));
    }

    #[test]
    fn test_unbounded_allocation_rejected() {
        // Declared string length of 10 MB in a 16-byte parcel
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&(10_000_000i32).to_le_bytes());
        let mut reader = ParcelReader::new(&buf);
        assert_eq!(reader.read_string8(), Err(ParcelError::UnboundedLength(10_000_000)));

        // Declared byte array of 10 MB
        let mut reader2 = ParcelReader::new(&buf);
        assert_eq!(reader2.read_byte_array(), Err(ParcelError::UnboundedLength(10_000_000)));
    }

    #[test]
    fn test_arithmetic_overflow_protection() {
        // Declared UTF-16 count near i32::MAX would overflow `count * 2`
        let mut buf = [0u8; 16];
        buf[0..4].copy_from_slice(&(0x7FFF_FFFFi32).to_le_bytes());
        let mut reader = ParcelReader::new(&buf);
        assert_eq!(reader.read_string16(), Err(ParcelError::UnboundedLength(0x7FFF_FFFF)));
    }

    #[test]
    fn test_truncated_buffer_overread_prevention() {
        // String declared length = 8, but only 4 payload bytes present
        let mut buf = [0u8; 8];
        buf[0..4].copy_from_slice(&(8i32).to_le_bytes());
        buf[4..8].copy_from_slice(b"test");
        let mut reader = ParcelReader::new(&buf);
        assert_eq!(reader.read_string8(), Err(ParcelError::UnboundedLength(8)));

        // Buffer truncated right inside padding
        let mut buf2 = [0u8; 9];
        buf2[0..4].copy_from_slice(&(2i32).to_le_bytes());
        buf2[4..6].copy_from_slice(b"hi");
        buf2[6] = 0; // null terminator
        let mut reader2 = ParcelReader::new(&buf2[..7]);
        assert_eq!(reader2.read_string8(), Err(ParcelError::BufferOverRead));
    }

    #[test]
    fn test_utf16_surrogate_pairs_and_null_terminator() {
        let mut buf = [0u8; 64];
        let mut writer = ParcelWriter::new(&mut buf);
        writer.write_string16_utf8("Hi 🎉!").unwrap();
        let written_len = writer.len();

        let mut reader = ParcelReader::new(&buf[..written_len]);
        let decoded = reader.read_string16().unwrap().unwrap();
        assert_eq!(decoded, "Hi 🎉!");
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn test_aidl_interface_enforce_and_security_denial() {
        let mut buf = [0u8; 128];
        let mut writer = ParcelWriter::new(&mut buf);
        writer.write_interface_token("android.location.ILocationManager").unwrap();
        let written_len = writer.len();

        // 1. Enforcing correct descriptor succeeds
        let mut reader_ok = ParcelReader::new(&buf[..written_len]);
        assert_eq!(reader_ok.enforce_interface("android.location.ILocationManager"), Ok(()));

        // 2. Enforcing mismatched descriptor fails closed
        let mut reader_bad = ParcelReader::new(&buf[..written_len]);
        assert_eq!(
            reader_bad.enforce_interface("android.os.IPowerManager"),
            Err(ParcelError::InterfaceMismatch)
        );
    }

    #[test]
    fn test_aidl_status_code_reply_roundtrip() {
        let mut buf = [0u8; 128];
        let mut writer = ParcelWriter::new(&mut buf);
        writer.write_exception(EX_SECURITY, "Permission Denial: Requires ACCESS_FINE_LOCATION").unwrap();
        let written_len = writer.len();

        let mut reader = ParcelReader::new(&buf[..written_len]);
        let exception_code = reader.read_i32().unwrap();
        assert_eq!(exception_code, EX_SECURITY);
        let msg = reader.read_string16().unwrap().unwrap();
        assert_eq!(msg, "Permission Denial: Requires ACCESS_FINE_LOCATION");
    }
}
