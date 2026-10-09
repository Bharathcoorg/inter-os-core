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

    /// Returns the total number of security violations intercepted and blocked.
    pub fn violation_count(&self) -> usize {
        self.audit_log.len()
    }
}

/// Global system-wide Android Binder mediator.
pub static BINDER_MEDIATOR: Mutex<BinderMediator> = Mutex::new(BinderMediator::new());
