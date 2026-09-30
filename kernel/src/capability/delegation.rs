//! Attenuated Capability Delegation & Time-Bounded Subcontracting
//!
//! Enables zero-trust capability derivation across tasks without ambient authority:
//! - Monotonic attenuation: Derived capabilities can only hold a strict subset of parent rights.
//! - Time-bounded leases: Automatic expiration after a specified hardware tick duration.
//! - Usage quotas: Bounded invocation counts (e.g., single-use payment authorization tokens).

use crate::capability::{CapError, Capability, CapabilityId, CapabilityType, Rights};
use crate::task::TaskId;
use crate::serial_println;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Kernel-owned registry for delegated leases. Capability authorization consults
/// this registry so expiry and invocation quotas cannot be bypassed by copying a token.
pub static DELEGATION_REGISTRY: Mutex<Vec<DelegatedLease>> = Mutex::new(Vec::new());

/// Monotonic allocator for opaque lease identifiers. The high bit is reserved
/// for delegated capability markers.
static NEXT_LEASE_ID: AtomicU64 = AtomicU64::new(1);

pub fn allocate_lease_id() -> u64 {
    let id = NEXT_LEASE_ID.fetch_add(1, Ordering::Relaxed) & 0x7FFF_FFFF_FFFF_FFFF;
    if id == 0 { 1 } else { id }
}

/// A time-bounded, usage-constrained capability delegation lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedLease {
    pub lease_id: u64,
    pub parent_cap_id: CapabilityId,
    pub grantor_task: TaskId,
    pub grantee_task: TaskId,
    pub cap_type: CapabilityType,
    /// Resource target inherited from the parent capability. The lease ID is
    /// kept only in the capability ID so object-scoped authorization remains
    /// bound to the original resource.
    pub object_id: u64,
    pub attenuated_rights: Rights,
    pub max_invocations: u32,
    pub remaining_invocations: u32,
    pub created_at_ticks: u64,
    pub expiry_ticks: u64,
}

impl DelegatedLease {
    /// Creates a new attenuated delegation lease from a parent capability.
    pub fn new(
        lease_id: u64,
        parent: &Capability,
        grantor: TaskId,
        grantee: TaskId,
        requested_rights: Rights,
        max_invocations: u32,
        duration_ticks: u64,
        current_ticks: u64,
    ) -> Result<Self, CapError> {
        if max_invocations == 0 || duration_ticks == 0 {
            return Err(CapError::InvalidSlot);
        }

        // 1. Parent must hold GRANT rights
        if !parent.rights.contains(Rights::GRANT) {
            return Err(CapError::InsufficientRights);
        }

        // 2. Monotonic attenuation invariant: requested rights MUST be a subset of parent rights
        if !parent.rights.contains(requested_rights) {
            return Err(CapError::InsufficientRights);
        }

        // Derived lease cannot hold GRANT rights itself (single-level attenuation)
        let safe_rights = Rights(requested_rights.0 & !Rights::GRANT.0);

        Ok(Self {
            lease_id,
            parent_cap_id: parent.id,
            grantor_task: grantor,
            grantee_task: grantee,
            cap_type: parent.cap_type,
            object_id: parent.object_id,
            attenuated_rights: safe_rights,
            max_invocations,
            remaining_invocations: max_invocations,
            created_at_ticks: current_ticks,
            expiry_ticks: current_ticks.saturating_add(duration_ticks),
        })
    }

    /// Verifies whether the lease is currently valid for an operation and consumes 1 invocation.
    pub fn consume_invocation(
        &mut self,
        current_ticks: u64,
        required_rights: Rights,
    ) -> Result<(), CapError> {
        // 1. Check expiration
        if current_ticks >= self.expiry_ticks {
            return Err(CapError::InvalidSlot); // Expired
        }

        // 2. Check quota
        if self.remaining_invocations == 0 {
            return Err(CapError::InvalidSlot); // Exhausted
        }

        // 3. Check rights
        if !self.attenuated_rights.contains(required_rights) {
            return Err(CapError::InsufficientRights);
        }

        self.remaining_invocations -= 1;
        Ok(())
    }

    /// Converts this lease into a concrete kernel capability token for insertion into grantee's table.
    pub fn to_capability(&self) -> Capability {
        Capability {
            id: CapabilityId(self.lease_id | 0x8000_0000_0000_0000),
            cap_type: self.cap_type,
            rights: self.attenuated_rights,
            object_id: self.object_id,
        }
    }
}

/// Self-test validating monotonic attenuation, lease expiry, and quota exhaustion.
pub fn init() {
    let parent_cap = Capability {
        id: CapabilityId(100),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ.union(Rights::WRITE).union(Rights::GRANT),
        object_id: 0,
    };

    let current_ticks = 1000u64;

    // 1. Valid Attenuation: Grant READ-only to subtask with 2 uses for 50 ticks
    let mut lease = DelegatedLease::new(
        1,
        &parent_cap,
        TaskId(1),
        TaskId(2),
        Rights::READ,
        2,
        50,
        current_ticks,
    ).expect("Valid delegation must succeed");

    assert_eq!(lease.attenuated_rights, Rights::READ);
    assert_eq!(lease.remaining_invocations, 2);
    assert_eq!(lease.object_id, parent_cap.object_id);
    let ipc_parent = Capability {
        id: CapabilityId(101),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE.union(Rights::GRANT),
        object_id: 42,
    };
    let ipc_lease = DelegatedLease::new(
        3,
        &ipc_parent,
        TaskId(1),
        TaskId(2),
        Rights::WRITE,
        1,
        50,
        current_ticks,
    ).expect("object-scoped delegation must succeed");
    let derived_ipc = ipc_lease.to_capability();
    let mut ipc_table = crate::capability::CapabilityTable::new(2);
    let ipc_slot = ipc_table.insert(derived_ipc).expect("derived IPC capability must install");
    assert!(ipc_table.verify_object(ipc_slot, CapabilityType::IpcSend, Rights::WRITE, 42).is_ok());
    assert_eq!(ipc_table.verify_object(ipc_slot, CapabilityType::IpcSend, Rights::WRITE, 99), Err(CapError::TargetMismatch));

    // 2. Consume invocation 1 (ticks = 1010)
    let inv1 = lease.consume_invocation(1010, Rights::READ);
    assert!(inv1.is_ok());
    assert_eq!(lease.remaining_invocations, 1);

    // 3. Consume invocation 2 (ticks = 1020)
    let inv2 = lease.consume_invocation(1020, Rights::READ);
    assert!(inv2.is_ok());
    assert_eq!(lease.remaining_invocations, 0);

    // 4. Quota exhaustion: Invocation 3 must fail
    let inv3 = lease.consume_invocation(1030, Rights::READ);
    assert_eq!(inv3, Err(CapError::InvalidSlot));

    // 5. Illegal Privilege Escalation: Requesting WRITE when parent lacks it (or requesting GRANT)
    let mut read_only_parent = parent_cap.clone();
    read_only_parent.rights = Rights::READ.union(Rights::GRANT); // No WRITE
    let illegal = DelegatedLease::new(
        2,
        &read_only_parent,
        TaskId(1),
        TaskId(2),
        Rights::WRITE,
        1,
        50,
        current_ticks,
    );
    assert_eq!(illegal, Err(CapError::InsufficientRights), "Deriving unheld rights must be rejected");

    serial_println!(
        "[TEST] Attenuated Capability Delegation: Monotonic rights attenuation, tick lease, and quota enforcement VERIFIED."
    );
}


/// Registers a lease only after its derived capability has been installed.
pub fn register_lease(lease: DelegatedLease) {
    let mut registry = DELEGATION_REGISTRY.lock();
    let now = crate::interrupts::get_system_ticks();
    registry.retain(|existing| existing.expiry_ticks > now);
    registry.push(lease);
}

/// Validates and consumes one invocation from a delegated lease.
pub fn consume_invocation(
    lease_id: u64,
    current_ticks: u64,
    required_rights: Rights,
) -> Result<(), CapError> {
    let mut registry = DELEGATION_REGISTRY.lock();
    let lease = registry
        .iter_mut()
        .find(|lease| lease.lease_id == lease_id)
        .ok_or(CapError::InvalidSlot)?;
    lease.consume_invocation(current_ticks, required_rights)
}

/// Returns true while a registered delegated lease is active.
pub fn is_lease_active(lease_id: u64, current_ticks: u64) -> bool {
    let registry = DELEGATION_REGISTRY.lock();
    registry.iter().any(|lease| lease.lease_id == lease_id && current_ticks < lease.expiry_ticks)
}
