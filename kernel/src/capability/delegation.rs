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
    /// Set when the source capability or this lease was revoked.
    ///
    /// WHY this is state and not only an absent registry entry: revocation must
    /// be observable as a distinct refusal, and a lease that was dropped from
    /// the registry is indistinguishable from one that never existed. Keeping
    /// the flag on the lease means the check below cannot be skipped by a
    /// registry that was rebuilt.
    pub revoked: bool,
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
            revoked: false,
        })
    }

    /// Kills this lease.
    ///
    /// WHY the quota is also emptied: a revoked lease must not be usable again
    /// even if a caller reads `remaining_invocations` as evidence of authority
    /// instead of asking the registry.
    pub fn revoke(&mut self) {
        self.revoked = true;
        self.remaining_invocations = 0;
    }

    /// Verifies whether the lease is currently valid for an operation and consumes 1 invocation.
    pub fn consume_invocation(
        &mut self,
        caller: Option<TaskId>,
        current_ticks: u64,
        required_rights: Rights,
    ) -> Result<(), CapError> {
        // Enforce cross-process isolation: if caller is known, it must match grantee_task!
        if let Some(caller_task) = caller {
            if caller_task != self.grantee_task {
                return Err(CapError::TargetMismatch);
            }
        }

        // 0. Revocation is checked first and unconditionally. A revoked lease
        // authorizes nothing, whatever its expiry and remaining quota say, and
        // the refusal is distinguishable from an exhausted quota so a caller can
        // tell "authority was taken back" from "authority ran out".
        if self.revoked {
            return Err(CapError::RevokedHandle);
        }

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
            id: lease_capability_id(self.lease_id),
            cap_type: self.cap_type,
            rights: self.attenuated_rights,
            object_id: self.object_id,
        }
    }
}

/// The identity a lease mints. The high bit marks a delegated capability, and
/// it is also how a revocation published against a lease is recognised as
/// belonging to that lease.
pub fn lease_capability_id(lease_id: u64) -> CapabilityId {
    CapabilityId(lease_id | 0x8000_0000_0000_0000)
}

/// Self-test validating monotonic attenuation, lease expiry, and quota exhaustion.
pub fn init() {
    let parent_cap = Capability {
        id: CapabilityId(100),
        cap_type: CapabilityType::FileSystemWrite,
        rights: Rights::READ.union(Rights::WRITE).union(Rights::GRANT),
        object_id: 0,
    };

    let current_ticks = crate::interrupts::get_system_ticks();

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
        1000,
        current_ticks,
    ).expect("object-scoped delegation must succeed");
    register_lease(ipc_lease.clone());
    let derived_ipc = ipc_lease.to_capability();
    let mut ipc_table = crate::capability::CapabilityTable::new_for_task(TaskId(2), 2);
    let ipc_slot = ipc_table.insert(derived_ipc.clone()).expect("derived IPC capability must install");
    assert!(ipc_table.verify_object(ipc_slot, CapabilityType::IpcSend, Rights::WRITE, 42).is_ok());
    assert_eq!(ipc_table.verify_object(ipc_slot, CapabilityType::IpcSend, Rights::WRITE, 99), Err(CapError::TargetMismatch));

    // Adversarial: Cross-process invocation attempt by TaskId(3) on TaskId(2)'s delegated capability must be rejected
    let mut rogue_table = crate::capability::CapabilityTable::new_for_task(TaskId(3), 2);
    let rogue_slot = rogue_table.insert(derived_ipc).expect("capability slot insertion succeeds");
    assert_eq!(
        rogue_table.verify_object(rogue_slot, CapabilityType::IpcSend, Rights::WRITE, 42),
        Err(CapError::TargetMismatch),
        "Cross-process invocation of delegated capability must be strictly rejected"
    );

    // 2. Consume invocation 1 (ticks = current_ticks + 10) by authorized grantee TaskId(2)
    let inv1 = lease.consume_invocation(Some(TaskId(2)), current_ticks + 10, Rights::READ);
    assert!(inv1.is_ok());
    assert_eq!(lease.remaining_invocations, 1);

    // Adversarial: Cross-process lease invocation by unauthorized caller TaskId(99)
    let rogue_inv = lease.consume_invocation(Some(TaskId(99)), current_ticks + 15, Rights::READ);
    assert_eq!(rogue_inv, Err(CapError::TargetMismatch), "Cross-process lease invocation must be rejected");

    // 3. Consume invocation 2 (ticks = current_ticks + 20) by authorized grantee TaskId(2)
    let inv2 = lease.consume_invocation(Some(TaskId(2)), current_ticks + 20, Rights::READ);
    assert!(inv2.is_ok());
    assert_eq!(lease.remaining_invocations, 0);

    // 4. Quota exhaustion: Invocation 3 must fail
    let inv3 = lease.consume_invocation(Some(TaskId(2)), current_ticks + 30, Rights::READ);
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

    DELEGATION_REGISTRY.lock().clear();

    serial_println!(
        "[TEST] Attenuated Capability Delegation: Monotonic rights attenuation, tick lease, and quota enforcement VERIFIED."
    );
}


/// Registers a lease only after its derived capability has been installed.
pub fn register_lease(lease: DelegatedLease) {
    let mut registry = DELEGATION_REGISTRY.lock();
    // WHY prune here rather than only on expiry: a lease with no invocations
    // left, or one that has been revoked, can never authorize again, so keeping
    // it only delays reclaiming it until its expiry. The registry is kernel
    // state about authority that nothing can use, and it must not grow without
    // bound.
    prune_locked(&mut registry, crate::interrupts::get_system_ticks());
    registry.push(lease);
}

/// Drops every lease that can no longer authorize anything.
fn prune_locked(registry: &mut Vec<DelegatedLease>, now: u64) -> usize {
    let before = registry.len();
    registry.retain(|lease| !lease.revoked && lease.expiry_ticks > now && lease.remaining_invocations > 0);
    before - registry.len()
}

/// Reclaims unusable leases. Callers on a hot path may use this to bound growth.
pub fn prune(now: u64) -> usize {
    prune_locked(&mut DELEGATION_REGISTRY.lock(), now)
}

/// Revokes one lease by identifier.
pub fn revoke_lease(lease_id: u64) -> bool {
    let mut registry = DELEGATION_REGISTRY.lock();
    let mut found = false;
    for lease in registry.iter_mut() {
        if lease.lease_id == lease_id && !lease.revoked {
            lease.revoke();
            found = true;
        }
    }
    found
}

/// Revokes every lease derived from `parent_cap_id`, returning the revoked lease ids.
///
/// WHY this is the whole point of a delegation record: the derived capability is
/// a copy of the source's authority. If the source is revoked and the copies
/// stay live, the revocation never reaches the processes that actually hold the
/// authority.
pub fn revoke_derived_from(parent_cap_id: CapabilityId) -> Vec<u64> {
    let mut registry = DELEGATION_REGISTRY.lock();
    let mut revoked = Vec::new();
    for lease in registry.iter_mut() {
        if lease.parent_cap_id == parent_cap_id && !lease.revoked {
            lease.revoke();
            revoked.push(lease.lease_id);
        }
    }
    revoked
}

/// Revokes every lease held by `grantee`, returning the revoked lease ids.
///
/// Used when a task exits. The lease outlives the task otherwise, and a lease
/// naming a task that no longer exists is authority nobody can account for.
pub fn revoke_for_grantee(grantee: TaskId) -> Vec<u64> {
    let mut registry = DELEGATION_REGISTRY.lock();
    let mut revoked = Vec::new();
    for lease in registry.iter_mut() {
        if lease.grantee_task == grantee && !lease.revoked {
            lease.revoke();
            revoked.push(lease.lease_id);
        }
    }
    revoked
}

/// Returns whether a registered delegated lease is still active: present, not
/// revoked, and unexpired.
pub fn is_lease_active(lease_id: u64, current_ticks: u64) -> bool {
    let registry = DELEGATION_REGISTRY.lock();
    registry.iter().any(|lease| {
        lease.lease_id == lease_id && !lease.revoked && current_ticks < lease.expiry_ticks
    })
}

/// Validates and consumes one invocation from a delegated lease.
pub fn consume_invocation(
    lease_id: u64,
    caller: Option<TaskId>,
    current_ticks: u64,
    required_rights: Rights,
) -> Result<(), CapError> {
    let mut registry = DELEGATION_REGISTRY.lock();
    let lease = registry
        .iter_mut()
        .find(|lease| lease.lease_id == lease_id)
        .ok_or(CapError::InvalidSlot)?;
    lease.consume_invocation(caller, current_ticks, required_rights)
}
