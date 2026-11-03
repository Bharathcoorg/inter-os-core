// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Authoritative, kernel-owned capability revocation state.
//!
//! Every authorization decision in this subsystem consults the registry below
//! instead of trusting the caller's copy of a capability.
//!
//! WHY the registry exists rather than only clearing a table slot: a
//! `Capability` is a plain `Clone`-able struct. It is copied into capability
//! tables, handed across the syscall boundary by handle, and transferred
//! in-band inside IPC messages. Clearing one slot in one table therefore
//! cannot be the enforcement point, because a second copy of the same
//! `CapabilityId` presented anywhere else would still authorize. Revocation is
//! therefore recorded against the *identity* of the capability, and every check
//! site asks this registry.
//!
//! ## Lifetime of a revoked capability
//!
//! * A delegated capability is authorized against the lease registry at the
//!   instant of use, and the lease's invocation counter is decremented *before*
//!   the operation runs. Revocation cannot be outrun by an in-flight call: an
//!   operation that has not yet reached its check finds the lease revoked and
//!   fails, and an operation that already consumed its invocation was
//!   authorized before the revocation by definition.
//! * A `CapHandle` carries its slot's generation. Revocation bumps the
//!   generation, so a handle held across the revocation resolves to
//!   `StaleHandle`. A *different* capability later installed in that slot gets a
//!   different handle value, so an old handle can never be replayed against it.
//! * A `Capability` value copied elsewhere is denied by identity, because the
//!   check consults `CapabilityId` and not the slot it happens to sit in.
//!
//! ## What is deliberately absent
//!
//! There is no time-based expiry of a revocation, and entries are never evicted
//! to bound this list. Either would silently restore authority: a capability
//! must stay dead until something explicitly re-establishes it. The only
//! reclaim path is `reinstate`, which an explicit fresh user consent resolution
//! calls.
//!
//! What is bounded instead is the *shape* the record is kept in: the store holds
//! disjoint ascending ranges of identifiers, so a task that revokes a run of
//! consecutive identities occupies one range rather than one entry each. See
//! [`publish`].

use alloc::vec::Vec;
use spin::Mutex;

use super::{delegation, CapabilityId, CapabilityType};
use crate::task::TaskId;

/// Revoked capability identifiers, as disjoint ascending `(start, end)` inclusive
/// ranges.
static REVOKED: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// Ephemeral capabilities currently suspended because their owning application/surface
/// has moved to the background or into the task switcher.
static EPHEMERAL_SUSPENDED: Mutex<Vec<(TaskId, CapabilityId)>> = Mutex::new(Vec::new());

/// Explicit tasks granted background authorization pills/privileges.
static BACKGROUND_PILL_TASKS: Mutex<Vec<TaskId>> = Mutex::new(Vec::new());

/// Publishes a revocation for `cap_id` and kills every capability derived from
/// it. Returns true when this call was the one that published it.
///
/// Idempotent: publishing the same revocation twice is not an error and changes
/// nothing, so the enforcement does not depend on how many times a revocation
/// path runs.
pub fn revoke(cap_id: CapabilityId) -> bool {
    let published = publish(cap_id);
    // WHY delegation is revoked here and not at the call site: a derived
    // capability is a copy of the source's authority attenuated by a lease. If
    // the source dies and the copy stays live, the grantee keeps the authority
    // the revocation was supposed to take back. Both the lease and the identity
    // it minted are published, because the identity is what an inspection-only
    // check consults and the lease is what an attenuated check consumes.
    for lease_id in delegation::revoke_derived_from(cap_id) {
        publish(delegation::lease_capability_id(lease_id));
    }
    published
}

/// Records a revocation without touching the delegation registry.
///
/// WHY the split: `revoke` recurses into the delegation registry, and a lease
/// revocation has to publish the identity that lease minted. Without a
/// non-recursive publish that would call itself.
///
/// WHY a range store rather than one entry per identity: identities come off one
/// monotonic per-boot counter, so the ones an exiting task publishes are a
/// contiguous run. Stored individually, every app that ever ran left its whole
/// capability table behind as permanent heap - which the lifecycle stress suite
/// measured as unbounded growth across relaunches - and every authorization
/// check had to scan all of it. Folded into ranges, one exited task costs one
/// range, and the store is bounded by the runs that still-live identities split
/// it into, not by the number of launches. Nothing is dropped, so no revocation
/// can be outrun.
fn publish(cap_id: CapabilityId) -> bool {
    fold(&mut REVOKED.lock(), cap_id.0)
}

/// Tests membership by locating the one range that can cover the identifier.
fn contains_revoked(id: u64) -> bool {
    covers(&REVOKED.lock(), id)
}

/// Adds one identifier to a set of disjoint ascending inclusive ranges, widening
/// or joining the ranges it touches, and reports whether it was not already in
/// the set.
///
/// The ranges are kept disjoint *and* non-touching, which is what makes
/// [`covers`] a binary search rather than a scan: if two stored ranges were
/// adjacent, a membership test could not tell a gap from a boundary.
fn fold(ranges: &mut Vec<(u64, u64)>, id: u64) -> bool {
    // Ranges ascend, so every range before `at` ends below `id` and every range
    // from `at` on ends at or above it.
    let at = ranges.partition_point(|&(_, end)| end < id);
    if ranges.get(at).is_some_and(|&(start, _)| start <= id) {
        return false;
    }
    // `id` may be adjacent to the range below, the range above, or both.
    // Neither `+ 1` can overflow: a range ending below `id` ends at most at
    // `id - 1`, and a range starting above `id` starts at most at `u64::MAX`,
    // so `id` itself is below `u64::MAX` whenever it is compared that way.
    let joins_below = at > 0 && ranges[at - 1].1 + 1 == id;
    let joins_above = ranges.get(at).is_some_and(|&(start, _)| start == id + 1);
    match (joins_below, joins_above) {
        (true, true) => {
            let upper_end = ranges[at].1;
            ranges[at - 1].1 = upper_end;
            ranges.remove(at);
        }
        (true, false) => ranges[at - 1].1 = id,
        (false, true) => ranges[at].0 = id,
        (false, false) => ranges.insert(at, (id, id)),
    }
    true
}

/// Removes one identifier from the range set, splitting the range it sat in, and
/// reports whether it was there.
fn unfold(ranges: &mut Vec<(u64, u64)>, id: u64) -> bool {
    let at = ranges.partition_point(|&(_, end)| end < id);
    let Some(&(start, end)) = ranges.get(at) else { return false; };
    if start > id {
        return false;
    }
    if start == end {
        ranges.remove(at);
    } else if start == id {
        ranges[at].0 = id + 1;
    } else if end == id {
        ranges[at].1 = id - 1;
    } else {
        // One identity is lifted out of the middle: the authority either side of
        // it was revoked separately from this one and stays revoked.
        ranges[at].1 = id - 1;
        ranges.insert(at + 1, (id + 1, end));
    }
    true
}

/// Reports whether `id` falls inside any stored range.
fn covers(ranges: &[(u64, u64)], id: u64) -> bool {
    let at = ranges.partition_point(|&(_, end)| end < id);
    ranges.get(at).is_some_and(|&(start, _)| start <= id)
}

/// Returns whether `cap_id` has been revoked or ephemerally suspended.
/// This is the authoritative question every authorization check must ask.
pub fn is_revoked(cap_id: CapabilityId) -> bool {
    if contains_revoked(cap_id.0) {
        return true;
    }
    EPHEMERAL_SUSPENDED.lock().iter().any(|(_, id)| *id == cap_id)
}

/// Returns whether `cap_id` is specifically in the ephemerally suspended state.
pub fn is_ephemeral_suspended(cap_id: CapabilityId) -> bool {
    EPHEMERAL_SUSPENDED.lock().iter().any(|(_, id)| *id == cap_id)
}

/// Grants or revokes an explicit background pill authorization to a task.
pub fn set_background_pill(task: TaskId, active: bool) {
    let mut pills = BACKGROUND_PILL_TASKS.lock();
    if active {
        if !pills.contains(&task) {
            pills.push(task);
        }
    } else {
        pills.retain(|t| *t != task);
    }
}

/// Checks if a task possesses an active explicit background pill authorization.
pub fn has_background_pill(task: TaskId) -> bool {
    BACKGROUND_PILL_TASKS.lock().contains(&task)
}

/// Suspends ephemeral background capabilities (camera, microphone, network DMA)
/// for `task_id` when it is moved into the background or swiped into recents.
/// If `has_background_pill(task_id)` is true, network access may remain active,
/// but hardware camera and microphone are always strictly suspended.
pub fn suspend_ephemeral_for_task(task_id: TaskId) -> usize {
    let bg_pill = has_background_pill(task_id);
    let mut suspended_count = 0;
    let mut caps_to_suspend = Vec::new();

    // 1. Synthesize deterministic permission capability IDs
    use crate::capability::permission::SensitivePermission;
    caps_to_suspend.push(SensitivePermission::CameraCapture.capability_id(task_id));
    caps_to_suspend.push(SensitivePermission::Microphone.capability_id(task_id));
    if !bg_pill {
        caps_to_suspend.push(SensitivePermission::NetworkSend.capability_id(task_id));
    }

    // 2. Also inspect the task's capability table for direct hardware port / net tokens
    if let Some(sched) = crate::task::SCHEDULER.try_lock() {
        if let Some(task) = sched.get_task(task_id) {
            for slot in 0..task.cap_table.capacity() {
                if let Some(cap) = task.cap_table.raw_slot(slot) {
                    let is_target = match cap.cap_type {
                        CapabilityType::HardwarePortIo => {
                            // Camera (0xCA00), Microphone (0xCA01)
                            cap.object_id == 0xCA00 || cap.object_id == 0xCA01
                        }
                        CapabilityType::NetworkSocket | CapabilityType::NetworkSend | CapabilityType::NetworkReceive => {
                            !bg_pill
                        }
                        CapabilityType::ShmProducer | CapabilityType::ShmConsumer => {
                            // Camera / audio DMA shm channels
                            !bg_pill && (cap.object_id == 0xCA00 || cap.object_id == 0xCA01)
                        }
                        _ => false,
                    };
                    if is_target && !caps_to_suspend.contains(&cap.id) {
                        caps_to_suspend.push(cap.id);
                    }
                }
            }
        }
    }

    let mut suspended = EPHEMERAL_SUSPENDED.lock();
    for cap_id in caps_to_suspend {
        if !suspended.iter().any(|(t, id)| *t == task_id && *id == cap_id) {
            suspended.push((task_id, cap_id));
            suspended_count += 1;
        }
    }

    if suspended_count > 0 {
        crate::serial_println!(
            "[PRIVACY] Suspended {} ephemeral background capability token(s) for task={}",
            suspended_count, task_id.0
        );
    }
    suspended_count
}

/// Reinstates ephemeral capabilities when `task_id` returns to the foreground.
pub fn reinstate_ephemeral_for_task(task_id: TaskId) -> usize {
    let mut suspended = EPHEMERAL_SUSPENDED.lock();
    let before = suspended.len();
    suspended.retain(|(t, _)| *t != task_id);
    let restored = before.saturating_sub(suspended.len());
    if restored > 0 {
        crate::serial_println!(
            "[PRIVACY] Reinstated {} ephemeral foreground capability token(s) for task={}",
            restored, task_id.0
        );
    }
    restored
}

/// Cleans up ephemeral state and background pills when a task terminates.
pub fn cleanup_task(task_id: TaskId) {
    EPHEMERAL_SUSPENDED.lock().retain(|(t, _)| *t != task_id);
    BACKGROUND_PILL_TASKS.lock().retain(|t| *t != task_id);
}

/// Lifts a revocation. Only an explicit re-grant of consent may call this.
///
/// WHY this is reachable at all: the capability identifier a permission mints
/// is a deterministic function of the task and the permission, so a re-grant
/// re-mints the same identifier. Without a way to lift a revocation, a granted-
/// then-revoked permission could never be used again, which would trade a
/// security property for an availability one. Lifting is tied to resolving an
/// outstanding consent request, never to the mere passage of time.
pub fn reinstate(cap_id: CapabilityId) -> bool {
    unfold(&mut REVOKED.lock(), cap_id.0)
}

/// Revokes a delegated lease directly, for a capability that lives in the lease
/// registry rather than in a table.
pub fn revoke_lease(lease_id: u64) -> bool {
    let revoked = delegation::revoke_lease(lease_id);
    publish(delegation::lease_capability_id(lease_id));
    revoked
}

/// Revokes every delegated lease a task holds. Used on the task exit path.
///
/// WHY publishing here is the half that has to be permanent: the lease record
/// itself is prunable (the delegation registry keeps only leases that can still
/// authorize, so a revoked lease is dropped from it), and a dropped record can
/// no longer answer a later `consume_invocation` as `RevokedHandle`. The
/// entry published below is never evicted, so the revocation of the identity the
/// lease minted survives the pruning - which is what every capability-table
/// check consults before it would consume an invocation. The caller therefore
/// still learns that authority was taken back, from the one store that is
/// designed to keep that answer.
pub fn revoke_grantee_delegations(grantee: TaskId) -> usize {
    let leases = delegation::revoke_for_grantee(grantee);
    for lease_id in &leases {
        publish(delegation::lease_capability_id(*lease_id));
    }
    leases.len()
}

/// Number of published revocations. Diagnostics only.
pub fn revoked_count() -> usize {
    REVOKED.lock().iter().fold(0usize, |total, &(start, end)| {
        total.saturating_add((end - start + 1) as usize)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
    use crate::task::TaskId;

    fn cap(id: u64, cap_type: CapabilityType) -> Capability {
        Capability { id: CapabilityId(id), cap_type, rights: Rights::WRITE, object_id: 7 }
    }

    /// The defect this closes: `revoke_by_id` cleared one slot of one table and
    /// returned a `bool` the only production caller discarded, so a second copy
    /// of the same capability kept working after revocation.
    ///
    /// The proving assertion is the second one: the *other* table, which was
    /// never touched by the revocation call, can no longer authorize the
    /// capability.
    #[test]
    fn revocation_reaches_every_copy_of_a_capability() {
        let mut granted = CapabilityTable::new_for_task(TaskId(41), 4);
        let mut retained_copy = CapabilityTable::new_for_task(TaskId(42), 4);
        let cap_id = CapabilityId(0x5A11_0001);
        let slot = granted.insert(cap(cap_id.0, CapabilityType::IpcSend)).expect("slot");
        let stale = retained_copy.insert(cap(cap_id.0, CapabilityType::IpcSend)).expect("slot");

        assert!(granted.verify_object(slot, CapabilityType::IpcSend, Rights::WRITE, 7).is_ok());
        assert!(retained_copy.authorize_object(CapabilityType::IpcSend, Rights::WRITE, 7));

        granted.revoke_by_id(cap_id);

        assert_eq!(
            retained_copy.verify_object(stale, CapabilityType::IpcSend, Rights::WRITE, 7),
            Err(crate::capability::CapError::RevokedHandle),
            "a retained copy must not authorize after the capability is revoked",
        );
        assert!(!retained_copy.authorize_object(CapabilityType::IpcSend, Rights::WRITE, 7));
        assert!(!retained_copy.has_capability(CapabilityType::IpcSend, Rights::WRITE));
        assert_eq!(retained_copy.find_slot(CapabilityType::IpcSend, Rights::WRITE), None);
        assert_eq!(retained_copy.count(), 0);
    }

    /// Revocation is enforced against every check shape the kernel uses. The
    /// table under test keeps its stale copy on purpose; the revocation is
    /// published from a second table, so these assertions are about the checks
    /// consulting the registry rather than about a slot having been cleared.
    #[test]
    fn every_check_shape_honours_revocation() {
        let cap_id = CapabilityId(0x5A11_0002);
        let mut table = CapabilityTable::new_for_task(TaskId(43), 4);
        let slot = table.insert(cap(cap_id.0, CapabilityType::FileSystemWrite)).expect("slot");
        let handle = table
            .insert_handle(cap(cap_id.0, CapabilityType::FileSystemWrite))
            .expect("handle");
        let mut revoker = CapabilityTable::new_for_task(TaskId(140), 2);
        revoker.insert(cap(cap_id.0, CapabilityType::FileSystemWrite)).expect("slot");

        assert!(table.has_capability(CapabilityType::FileSystemWrite, Rights::WRITE));
        assert_eq!(
            table.verify_object(slot, CapabilityType::FileSystemWrite, Rights::WRITE, 7),
            Ok(&cap(cap_id.0, CapabilityType::FileSystemWrite)),
        );

        assert!(revoker.revoke_by_id(cap_id));

        assert!(!table.has_capability(CapabilityType::FileSystemWrite, Rights::WRITE));
        assert_eq!(
            table.verify(slot, CapabilityType::FileSystemWrite, Rights::WRITE),
            Err(crate::capability::CapError::RevokedHandle),
        );
        assert_eq!(
            table.verify_object(slot, CapabilityType::FileSystemWrite, Rights::WRITE, 7),
            Err(crate::capability::CapError::RevokedHandle),
        );
        assert_eq!(
            table.verify_handle(handle, CapabilityType::FileSystemWrite, Rights::WRITE),
            Err(crate::capability::CapError::RevokedHandle),
        );
        assert_eq!(table.get(slot), Err(crate::capability::CapError::RevokedHandle));
        assert_eq!(
            table.get_by_handle(handle),
            Err(crate::capability::CapError::RevokedHandle),
        );
        assert!(!table.authorize_capability(CapabilityType::FileSystemWrite, Rights::WRITE));
        assert!(!table.authorize_object(CapabilityType::FileSystemWrite, Rights::WRITE, 7));
        assert_eq!(table.find_slot(CapabilityType::FileSystemWrite, Rights::WRITE), None);
        assert_eq!(table.count(), 0, "a revoked capability must not be counted as usable");
        assert!(table.validate_invariants());
    }

    /// Every slot holding the identity is cleared, not just the first.
    #[test]
    fn revoke_by_id_clears_every_duplicate_slot() {
        let cap_id = CapabilityId(0x5A11_0003);
        let mut table = CapabilityTable::new_for_task(TaskId(44), 6);
        for _ in 0..3 {
            table.insert(cap(cap_id.0, CapabilityType::NetworkSend)).expect("slot");
        }
        assert_eq!(table.count(), 3);
        assert!(table.revoke_by_id(cap_id));
        assert_eq!(table.count(), 0, "a duplicated capability must be revoked everywhere");
        assert!(table.validate_invariants());
    }

    /// A handle presented after revocation fails as stale, and a handle for a
    /// later capability installed in the same slot is a different value.
    #[test]
    fn handles_do_not_survive_revocation_or_reuse_the_slot() {
        let mut table = CapabilityTable::new_for_task(TaskId(45), 2);
        let handle = table.insert_handle(cap(0x5A11_0004, CapabilityType::SerialOutput)).expect("handle");
        assert!(table.get_by_handle(handle).is_ok());
        table.revoke_handle(handle).expect("revoke");
        assert_eq!(table.get_by_handle(handle), Err(crate::capability::CapError::StaleHandle));

        let replacement = table
            .insert_handle(cap(0x5A11_0005, CapabilityType::SerialOutput))
            .expect("replacement");
        assert_ne!(replacement, handle, "a re-used slot must not re-issue the old handle");
        assert!(table.get_by_handle(replacement).is_ok());
    }

    /// Revocation is idempotent and can only be lifted explicitly.
    #[test]
    fn revocation_is_idempotent_and_only_reinstate_lifts_it() {
        let cap_id = CapabilityId(0x5A11_0006);
        assert!(!is_revoked(cap_id));
        assert!(revoke(cap_id));
        assert!(is_revoked(cap_id));
        assert!(!revoke(cap_id), "a second publish is a no-op");
        assert!(is_revoked(cap_id));

        assert!(reinstate(cap_id));
        assert!(!is_revoked(cap_id));
        assert!(!reinstate(cap_id), "lifting a live capability is a no-op");
    }

    /// Revoking the source of a delegation kills the derived capability, so
    /// authority cannot be laundered through a grantee.
    #[test]
    fn revoking_a_source_kills_its_delegated_lease() {
        let parent_id = CapabilityId(0x5A11_0007);
        // WHY the grantee is drawn from the task counter instead of named: the lease
        // goes into the process-global `DELEGATION_REGISTRY` under its grantee, and
        // `capability::cleanup_task` -- which every task-exit path runs -- revokes
        // every lease whose grantee matches the departing task. One monotone counter
        // hands ids to every case in this suite, so a hand-picked grantee can be an id
        // an earlier `spawn` was handed, and that task's exit then strips the lease
        // this case is mid-flight on: the check below answers `RevokedHandle` and says
        // nothing about the stranger. A drawn id is one no other case can hold. The
        // grantor id stays a literal because `grantor_task` is stored and never read,
        // and its table is local to this case.
        let grantee_task = TaskId::new();
        let mut parent = CapabilityTable::new_for_task(TaskId(46), 2);
        let mut parent_cap = cap(parent_id.0, CapabilityType::IpcSend);
        parent_cap.rights = Rights::WRITE | Rights::GRANT;
        parent.insert(parent_cap).expect("slot");

        let lease = crate::capability::DelegatedLease::new(
            crate::capability::delegation::allocate_lease_id(),
            &{
                let mut borrowed = cap(parent_id.0, CapabilityType::IpcSend);
                borrowed.rights = Rights::WRITE | Rights::GRANT;
                borrowed
            },
            TaskId(46),
            grantee_task,
            Rights::WRITE,
            5,
            10_000,
            crate::interrupts::get_system_ticks(),
        )
        .expect("lease");
        crate::capability::delegation::register_lease(lease.clone());
        let derived_id = lease.to_capability().id;
        let mut grantee = CapabilityTable::new_for_task(grantee_task, 2);
        let slot = grantee.insert(lease.to_capability()).expect("slot");
        assert!(grantee.verify_object(slot, CapabilityType::IpcSend, Rights::WRITE, 7).is_ok());

        parent.revoke_by_id(parent_id);

        assert!(
            is_revoked(derived_id),
            "a capability derived from a revoked source must be revoked too",
        );
        assert_eq!(
            grantee.verify_object(slot, CapabilityType::IpcSend, Rights::WRITE, 7),
            Err(crate::capability::CapError::RevokedHandle),
            "the grantee must lose the delegated authority with its source",
        );
    }

    /// A revoked lease is inactive and unusable, and the identity it minted is
    /// revoked too, so an inspection-only check cannot pass it.
    #[test]
    fn revoked_lease_is_inactive_and_unusable() {
        let lease_id = crate::capability::delegation::allocate_lease_id();
        let now = crate::interrupts::get_system_ticks();
        // Same reason as in `revoking_a_source_kills_its_delegated_lease`: this
        // grantee is written into the global registry and is asked back by name at the
        // invocation check below, so a literal id lets another case's exit revoke the
        // lease -- or lets the lease this case registers be revoked on their behalf.
        let grantee_task = TaskId::new();
        let mut parent = cap(0x5A11_0008, CapabilityType::NetworkSend);
        parent.rights = Rights::WRITE | Rights::GRANT;
        let lease = crate::capability::DelegatedLease::new(
            lease_id,
            &parent,
            TaskId(48),
            grantee_task,
            Rights::WRITE,
            3,
            10_000,
            now,
        )
        .expect("lease");
        crate::capability::delegation::register_lease(lease.clone());
        assert!(crate::capability::delegation::is_lease_active(lease_id, now));

        assert!(revoke_lease(lease_id));

        assert!(!crate::capability::delegation::is_lease_active(lease_id, now));
        assert!(
            is_revoked(crate::capability::delegation::lease_capability_id(lease_id)),
            "the identity a revoked lease minted must be revoked as well",
        );
        assert_eq!(
            crate::capability::delegation::consume_invocation(
                lease_id,
                Some(grantee_task),
                now,
                Rights::WRITE
            ),
            Err(crate::capability::CapError::RevokedHandle),
        );
    }

    /// A task that exits takes its delegated authority with it. Nothing may be
    /// exercised under the dead task afterwards.
    ///
    /// WHY the registry call answers `InvalidSlot` and not `RevokedHandle`:
    /// `capability::cleanup_task` revokes the lease and then prunes the registry,
    /// so the record is gone by the time anything asks again and
    /// `consume_invocation` is reporting "there is no live lease for this
    /// identifier". Nothing is authorised by that answer. Demanding
    /// `RevokedHandle` here would mean keeping a tombstone for every lease ever
    /// revoked so one diagnostic survives, growing a kernel registry without
    /// bound - which is precisely what pruning exists to prevent, and no caller
    /// could observe the difference anyway: every capability error collapses to
    /// `PermissionDenied` at the syscall boundary.
    ///
    /// WHY that costs no information: the revocation itself is durable. `REVOKED`
    /// is never evicted, and every capability-table check consults it *before*
    /// an invocation is consumed - so a retained copy of the delegated capability
    /// is still denied as `RevokedHandle`, which the last assertion pins. The
    /// distinction the caller needs ("authority was taken back", not "authority
    /// ran out") is delivered by the store that is built to keep it.
    #[test]
    fn exiting_task_takes_its_delegations_with_it() {
        // WHY this task id is drawn from the counter instead of named:
        // `capability::cleanup_task` is not a local operation. For the id it is given
        // it revokes every lease in the global delegation registry naming that grantee,
        // prunes the registry, drops that task's consent records from the global
        // `PERMISSION_MANAGER`, and clears its ephemeral-suspension and background-pill
        // entries -- and each revocation it publishes lands in `REVOKED`, which is
        // never evicted. Hand-pick the id and it can be one a `spawn` in another case
        // was handed: this cleanup then strips a live case's authority, and the
        // reverse -- a stranger leaving records under the same id -- makes this cleanup
        // revoke more than the one lease the case created. Drawing the id takes it out
        // of everyone else's reach, which is also the only way this case can be sure
        // the exit it is simulating is its own.
        let gone = TaskId::new();
        let mut parent = cap(0x5A11_0009, CapabilityType::ShmProducer);
        parent.rights = Rights::WRITE | Rights::GRANT;
        let now = crate::interrupts::get_system_ticks();
        let lease = crate::capability::DelegatedLease::new(
            crate::capability::delegation::allocate_lease_id(),
            &parent,
            TaskId(0),
            gone,
            Rights::WRITE,
            4,
            10_000,
            now,
        )
        .expect("lease");
        crate::capability::delegation::register_lease(lease.clone());

        // The grantee keeps its own copy of the derived capability, which is the
        // shape a capability has when it is copied into an IPC message in flight.
        // It must work before the exit and fail after it.
        let mut grantee = CapabilityTable::new_for_task(gone, 2);
        let retained = grantee.insert(lease.to_capability()).expect("slot");
        assert!(grantee
            .verify_object(retained, CapabilityType::ShmProducer, Rights::WRITE, 7)
            .is_ok());

        crate::capability::cleanup_task(gone);

        assert!(
            !crate::capability::delegation::is_lease_active(lease.lease_id, now),
            "a lease granted to an exited task must not remain active",
        );
        assert!(is_revoked(crate::capability::delegation::lease_capability_id(lease.lease_id)));
        assert_eq!(
            crate::capability::delegation::consume_invocation(lease.lease_id, Some(gone), now, Rights::WRITE),
            Err(crate::capability::CapError::InvalidSlot),
            "a removed lease must not authorize an invocation; no live record, so no \
             capability to spend",
        );
        assert_eq!(
            grantee.verify_object(retained, CapabilityType::ShmProducer, Rights::WRITE, 7),
            Err(crate::capability::CapError::RevokedHandle),
            "a retained copy must still be denied as revoked, which is the durable part",
        );
        assert!(
            !grantee.authorize_object(CapabilityType::ShmProducer, Rights::WRITE, 7),
            "the inspecting check shape must refuse it too",
        );
    }

    /// Nothing leaks when a task exits: every capability it held is revoked by
    /// identity and its table is emptied.
    #[test]
    fn revoke_all_leaves_nothing_behind() {
        let mut table = CapabilityTable::new_for_task(TaskId(51), 4);
        let a = CapabilityId(0x5A11_000A);
        let b = CapabilityId(0x5A11_000B);
        table.insert(cap(a.0, CapabilityType::SerialOutput)).expect("slot");
        table.insert(cap(b.0, CapabilityType::TimerTickRead)).expect("slot");
        assert_eq!(table.count(), 2);

        table.revoke_all();

        assert_eq!(table.count(), 0);
        assert!(is_revoked(a));
        assert!(is_revoked(b));
        assert!(!table.authorize_capability(CapabilityType::SerialOutput, Rights::WRITE));
        assert!(table.validate_invariants());
    }

    /// The permission-minted id space and the delegated-lease id space must stay
    /// disjoint, because `verify_object` classifies a capability as a lease purely
    /// by testing bit 63 of its id.
    ///
    /// WHY this test exists: permission ids used the `0x9000_...` base, which sets
    /// bit 63, so `verify_object` routed every permission-minted capability into
    /// `delegation::consume_invocation` for a lease that was never registered. The
    /// result was `InvalidSlot`, and the kernel panicked inside its own boot
    /// self-test (`adversarial.app` camera grant). The host unit suite never caught
    /// it because that self-test only runs on real boot.
    ///
    /// The assertion is on the bit itself, because that bit is the whole mechanism:
    /// a test that merely verified a minted capability would pass for the wrong
    /// reason if the classifier were ever changed to consult the lease registry.
    #[test]
    fn permission_ids_are_not_mistaken_for_delegated_leases() {
        use crate::capability::permission::{SensitivePermission, PERMISSION_ID_BASE};
        use crate::capability::delegation::lease_capability_id;

        const LEASE_TAG: u64 = 0x8000_0000_0000_0000;

        assert_eq!(
            PERMISSION_ID_BASE & LEASE_TAG,
            0,
            "the permission id base must not set the delegated-lease tag bit"
        );

        // WHY the task is drawn from the counter instead of named: a permission id is
        // a deterministic function of `(task, permission)`, so naming the task names
        // four capability identities for the whole process. `verify_object` consults
        // the revoked store, which is never evicted, and `suspend_ephemeral_for_task`
        // writes into it by task -- so if the counter ever handed another case the
        // literal id this case picked, that case's backgrounding or exit would leave
        // these four identities revoked and this case would fail on an authority some
        // other task lost.
        let task = TaskId::new();
        let mut table = CapabilityTable::new_for_task(task, 4);

        for perm in [
            SensitivePermission::TelephonyModem,
            SensitivePermission::CameraCapture,
            SensitivePermission::Files,
            SensitivePermission::Sensors,
        ] {
            let minted = table
                .insert(perm.to_capability(task))
                .expect("insert minted capability");
            assert!(
                perm.capability_id(task).0 & LEASE_TAG == 0,
                "{:?} produced a capability id in the delegated-lease space",
                perm
            );
            assert!(
                table
                    .verify_object(
                        minted,
                        perm.to_capability(task).cap_type,
                        perm.to_capability(task).rights,
                        perm.to_capability(task).object_id,
                    )
                    .is_ok(),
                "{:?} must verify against the very capability it minted",
                perm
            );
        }

        // And the lease space must still classify as a lease.
        assert_ne!(
            lease_capability_id(7).0 & LEASE_TAG,
            0,
            "a delegated lease must keep its tag bit"
        );
    }

    /// A relaunched app must be born with usable capabilities.
    ///
    /// WHY this test exists: app spawners build capability tables from hardcoded
    /// constants, and `revoke_all` publishes those ids into a kernel-wide set that
    /// is never evicted. Without re-keying at the spawn funnel, the second launch of
    /// an app created a table in which every slot was already revoked, so
    /// `find_slot` returned `None`, `surface_create` was refused, and the app never
    /// presented a canvas. The visible symptom was "Recents failed to present canvas
    /// within timeout", which sent the investigation to the compositor and the GUI
    /// layer, where nothing was wrong.
    ///
    /// This is the only assertion that would have caught it: the surface and
    /// compositor path is provably healthy on a first launch.
    #[test]
    fn a_relaunched_app_is_not_born_with_revoked_capabilities() {
        // Mirrors `spawn_app_music`: the same constants on every launch.
        let build_table = |task: TaskId| {
            let mut t = CapabilityTable::new_for_task(task, 10);
            for (i, ctype) in [
                CapabilityType::SerialOutput,
                CapabilityType::IpcSend,
                CapabilityType::TimerTickRead,
                CapabilityType::HardwarePortIo,
            ]
            .iter()
            .enumerate()
            {
                t.insert(Capability {
                    id: CapabilityId(2601 + i as u64),
                    cap_type: *ctype,
                    rights: Rights::READ | Rights::WRITE,
                    object_id: 0,
                })
                .expect("slot");
            }
            t
        };

        // First launch: usable, because the spawn funnel re-keys every table it
        // builds (`task::user`), and this mirrors that.
        //
        // WHY re-keying is in the fixture rather than an assumption: the revoked set
        // is process-global and never evicted, and `CapabilityId(2601..2604)` are the
        // constants this case shares with
        // `unrekeyed_relaunch_reproduces_the_revoked_capability_defect`, which
        // publishes them into that set. Asserting the first launch is usable without
        // re-keying therefore only passed while this case happened to run first.
        let mut first = build_table(TaskId(24));
        assert_eq!(
            first.rekey_all(),
            4,
            "the spawn funnel re-keys every capability in a freshly built table"
        );
        let first_count = first.count();
        assert!(first_count > 0, "first launch must have usable capabilities");
        assert!(first.find_slot(CapabilityType::SerialOutput, Rights::WRITE).is_some());

        // The app runs, then exits. This is what poisons the constants.
        assert_eq!(first.revoke_all(), first_count);
        assert!(first.find_slot(CapabilityType::SerialOutput, Rights::WRITE).is_none());

        // Second launch: the spawn funnel re-keys, so the new table is usable again.
        let mut second = build_table(TaskId(31));
        second.rekey_all();
        assert_eq!(
            second.count(),
            first_count,
            "a relaunched app must keep every capability it was granted"
        );
        assert!(
            second.find_slot(CapabilityType::SerialOutput, Rights::WRITE).is_some(),
            "a relaunched app must be able to authorize, not be born revoked"
        );
        for ctype in [
            CapabilityType::SerialOutput,
            CapabilityType::IpcSend,
            CapabilityType::TimerTickRead,
            CapabilityType::HardwarePortIo,
        ] {
            assert!(
                second.find_slot(ctype, Rights::READ).is_some(),
                "{:?} must survive a relaunch",
                ctype
            );
        }
    }

    /// Without re-keying, the second launch is dead on arrival. This pins the defect
    /// itself rather than the fix, so a future change that removes `rekey_all` from
    /// the spawn path fails here with a clear cause.
    #[test]
    fn unrekeyed_relaunch_reproduces_the_revoked_capability_defect() {
        let build_table = || {
            let mut t = CapabilityTable::new_for_task(TaskId(24), 4);
            t.insert(cap(2601, CapabilityType::SerialOutput)).expect("slot");
            t.insert(cap(2602, CapabilityType::IpcSend)).expect("slot");
            t
        };

        let mut first = build_table();
        first.revoke_all();

        let second = build_table();
        // `count()` filters out revoked capabilities, so a table full of revoked
        // slots reports 0. That is exactly the `caps=0` observed for the relaunched
        // app in the boot log, and it is why the failure looked like a presentation
        // bug rather than an authorisation one.
        assert_eq!(
            second.count(),
            0,
            "without re-keying the relaunched app has zero authorizable capabilities"
        );
        assert!(
            second.find_slot(CapabilityType::SerialOutput, Rights::WRITE).is_none(),
            "without re-keying the relaunched table is born revoked -- this is the bug"
        );
        assert!(
            !second.validate_invariants() || second.find_slot(CapabilityType::IpcSend, Rights::WRITE).is_none(),
            "no capability in the unrekeyed relaunch may authorize"
        );
    }

    #[test]
    fn test_ephemeral_background_revocation_and_reinstatement() {
        // WHY the task is drawn from the counter instead of named: the suspension
        // store and the pill list are process-global and keyed by task id, and the
        // permission identities this case reads back are `(task, permission)`
        // derivations of exactly that id. A literal can be an id the shared counter
        // handed to some other case's `spawn`, and then two things go wrong at once:
        // the opening `!is_revoked` checks only hold while nobody else has suspended
        // that task, and `suspend_ephemeral_for_task` walks the scheduler entry the id
        // does have and suspends a live task's real camera, microphone and network
        // tokens. Drawing the id leaves the case with a task nobody else can reach, so
        // no lock is needed: every write below is keyed to an id only this case holds.
        let task = TaskId::new();
        use crate::capability::permission::SensitivePermission;
        let cam_id = SensitivePermission::CameraCapture.capability_id(task);
        let mic_id = SensitivePermission::Microphone.capability_id(task);
        let net_id = SensitivePermission::NetworkSend.capability_id(task);

        assert!(!is_revoked(cam_id));
        assert!(!is_revoked(mic_id));
        assert!(!is_revoked(net_id));

        // Background the task without background pill: camera, mic, and net are suspended
        let count = suspend_ephemeral_for_task(task);
        assert!(count >= 3);
        assert!(is_revoked(cam_id));
        assert!(is_revoked(mic_id));
        assert!(is_revoked(net_id));
        assert!(is_ephemeral_suspended(cam_id));

        // Reinstating foreground clears the ephemeral suspension
        let restored = reinstate_ephemeral_for_task(task);
        assert!(restored >= 3);
        assert!(!is_revoked(cam_id));
        assert!(!is_revoked(mic_id));
        assert!(!is_revoked(net_id));
        assert!(!is_ephemeral_suspended(cam_id));

        cleanup_task(task);
    }

    /// The store shape that bounds revocation memory: a run of consecutive
    /// identities occupies one range.
    ///
    /// WHY this is the test: an exiting task publishes the identities of its
    /// whole capability table, and those identities come off one monotonic
    /// counter, so they arrive as a run. Kept one entry per identity, the store
    /// grew by eight bytes for every capability of every app that ever ran, and
    /// the lifecycle stress suite measured it as heap that never came back.
    #[test]
    fn a_run_of_identities_folds_into_one_range() {
        let mut ranges = Vec::new();
        for id in 100..132 {
            assert!(fold(&mut ranges, id), "{id} was not revoked before");
            assert_eq!(ranges.len(), 1, "a contiguous run must stay one range");
            assert!(covers(&ranges, id));
        }
        assert!(!covers(&ranges, 99));
        assert!(!covers(&ranges, 132));
        assert!(!fold(&mut ranges, 115), "publishing a revoked identity again is a no-op");
        assert_eq!(ranges, vec![(100, 131)]);
    }

    /// Relaunching does not add to the store once the first exit has folded.
    #[test]
    fn repeated_launch_and_exit_cycles_do_not_add_ranges() {
        let mut ranges = Vec::new();
        let mut next_id = 0x0000_1000u64;
        for _ in 0..40 {
            // One spawn: the table's slots take the next identities in order.
            let run = next_id..next_id + 12;
            next_id += 12;
            // One exit: every identity in it is published.
            for id in run {
                fold(&mut ranges, id);
            }
        }
        assert_eq!(ranges.len(), 1, "40 exit-time runs became one revoked span");
        assert_eq!(ranges[0], (0x0000_1000, next_id - 1));
        assert!(covers(&ranges, 0x0000_1000));
        assert!(covers(&ranges, next_id - 1));
        assert!(!covers(&ranges, next_id), "nothing past the minted run is revoked");
    }

    /// A still-live identity keeps the revoked runs either side of it apart, and
    /// revoking it afterwards joins them back into one.
    #[test]
    fn a_live_identity_holds_two_runs_apart_and_folds_them_when_it_goes() {
        let mut ranges = Vec::new();
        for id in 200..210 {
            fold(&mut ranges, id);
        }
        // The next task is still running, so 210 is not revoked and the runs
        // below and above it cannot be joined.
        for id in 211..221 {
            fold(&mut ranges, id);
        }
        assert_eq!(ranges, vec![(200, 209), (211, 220)]);
        assert!(!covers(&ranges, 210));

        assert!(fold(&mut ranges, 210), "the gap was live until now");
        assert_eq!(ranges, vec![(200, 220)], "bridging a gap joins both sides");
    }

    /// Folding must not swallow an identity that was never revoked.
    #[test]
    fn folding_never_revokes_an_identity_beyond_the_one_published() {
        let mut ranges = Vec::new();
        fold(&mut ranges, 300);
        fold(&mut ranges, 302);
        assert_eq!(ranges, vec![(300, 300), (302, 302)]);
        for (id, expected) in [(299, false), (300, true), (301, false), (302, true), (303, false)] {
            assert_eq!(covers(&ranges, id), expected, "{id}");
        }
    }

    /// Lifting one identity out of a folded range leaves the rest revoked, so the
    /// range shape cannot make `reinstate` restore more authority than it was
    /// given.
    #[test]
    fn lifting_one_identity_out_of_a_folded_range_splits_it() {
        let mut ranges = Vec::new();
        for id in 400..410 {
            fold(&mut ranges, id);
        }
        assert!(unfold(&mut ranges, 405));
        assert_eq!(ranges, vec![(400, 404), (406, 409)]);
        assert!(!covers(&ranges, 405));
        assert!(covers(&ranges, 404));
        assert!(covers(&ranges, 406));
        assert!(!unfold(&mut ranges, 405), "lifting a live identity is a no-op");

        // The ends of a range behave the same way, and shrink rather than split.
        assert!(unfold(&mut ranges, 400));
        assert!(unfold(&mut ranges, 409));
        assert_eq!(ranges, vec![(401, 404), (406, 408)]);

        // A range of one is removed outright, so no empty range survives.
        let mut single = Vec::new();
        fold(&mut single, 7);
        assert!(unfold(&mut single, 7));
        assert!(single.is_empty());
        assert!(!unfold(&mut single, 7));
    }

    /// The edges of the identifier space must not be able to wrap a range.
    #[test]
    fn the_ends_of_the_identifier_space_stay_separate_ranges() {
        let mut ranges = Vec::new();
        fold(&mut ranges, u64::MIN);
        fold(&mut ranges, u64::MAX);
        assert_eq!(ranges, vec![(u64::MIN, u64::MIN), (u64::MAX, u64::MAX)]);
        assert!(covers(&ranges, u64::MIN));
        assert!(covers(&ranges, u64::MAX));
        assert!(!covers(&ranges, 1));

        // Widening a range to the very top must not overflow the adjacency test.
        fold(&mut ranges, u64::MAX - 1);
        assert_eq!(ranges, vec![(u64::MIN, u64::MIN), (u64::MAX - 1, u64::MAX)]);
    }

    #[test]
    fn test_background_pill_preserves_network_while_camera_mic_suspended() {
        // Drawn for the two reasons laid out in
        // `test_ephemeral_background_revocation_and_reinstatement`: the pill list is a
        // global keyed by task id, and the "network stays live" assertion below is only
        // about a task no other case can have backgrounded or handed a pill of its own.
        let task = TaskId::new();
        use crate::capability::permission::SensitivePermission;
        let cam_id = SensitivePermission::CameraCapture.capability_id(task);
        let mic_id = SensitivePermission::Microphone.capability_id(task);
        let net_id = SensitivePermission::NetworkSend.capability_id(task);

        // Grant background pill authorization
        set_background_pill(task, true);
        assert!(has_background_pill(task));

        // Background the task: camera and mic are strictly suspended, but network remains active!
        suspend_ephemeral_for_task(task);
        assert!(is_revoked(cam_id));
        assert!(is_revoked(mic_id));
        assert!(!is_revoked(net_id), "background pill must preserve network access");

        // Revoke background pill and cleanup
        set_background_pill(task, false);
        cleanup_task(task);
    }
}