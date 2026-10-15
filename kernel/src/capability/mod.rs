//! Inter OS Zero-Trust Capability Security Subsystem
//!
//! Enforces the fundamental security invariant: "No ambient authority; security
//! is enforced by the kernel below the AI and application layer."
//!
//! Every task must present a valid `Capability` token to perform privileged
//! operations (such as UART logging, direct framebuffer access, hardware timer
//! reading, IPC messaging, or spawning/terminating tasks).

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

/// Unique capability identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(pub u64);

/// Categorization of resources and operations guarded by capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityType {
    /// Permission to write diagnostic log messages to UART serial COM1.
    SerialOutput,
    /// Permission to directly manipulate the 1280x720 video framebuffer.
    FrameBufferDirect,
    /// Permission to read system hardware timer metrics (PIT / TSC).
    TimerTickRead,
    /// Permission to read/write hardware I/O ports (driver tasks only).
    HardwarePortIo,
    /// Permission to send an IPC message to a designated target task.
    IpcSend,
    /// Permission to receive incoming IPC messages.
    IpcReceive,
    /// Permission to spawn new child tasks.
    ProcessSpawn,
    /// Permission to terminate a task.
    ProcessKill,
    /// Permission to read files within the virtual filesystem.
    FileSystemRead,
    /// Permission to create or write files within the virtual filesystem.
    FileSystemWrite,
    /// Permission to perform raw block I/O on storage devices.
    StorageRawBlock,
    /// Permission to create and bind network sockets.
    NetworkSocket,
    /// Permission to transmit network packets.
    NetworkSend,
    /// Permission to receive network packets.
    NetworkReceive,
    /// Permission to map and write to shared memory zero-copy DMA ring buffers.
    ShmProducer,
    /// Permission to map and read from shared memory zero-copy DMA ring buffers.
    ShmConsumer,
    /// Permission to register for and poll asynchronous kernel events (epoll/kqueue reactor).
    EventPoll,
    /// Permission for privileged global kernel controls (power, crypto, telephony, audio, device policy).
    SystemControl,
    /// Permission to read the hardware real-time clock (date and time).
    ///
    /// WHY this exists rather than reusing `SystemControl`: reading the clock is a
    /// passive observation that grants no authority over anything, whereas
    /// `SystemControl` is documented as power, crypto, telephony, audio and device
    /// policy. `sys_rtc_get_time` used to require `SystemControl`, and the only
    /// guard for it was the object-blind `require_system_control()`, so satisfying
    /// it would have handed the caller power, crypto and telephony authority over
    /// the whole device just to learn the time. This file already documents why
    /// the object-blind form is wrong: see the account-administration guard.
    ///
    /// The lock screen is the reason this matters in practice. It could not render
    /// a clock without this capability, so the one element that dominates the
    /// reference design was permanently absent rather than merely wrong.
    RealtimeClockRead,
}

impl CapabilityType {
    pub const fn to_raw(self) -> u64 {
        match self {
            CapabilityType::SerialOutput => 1,
            CapabilityType::FrameBufferDirect => 2,
            CapabilityType::TimerTickRead => 3,
            CapabilityType::HardwarePortIo => 4,
            CapabilityType::IpcSend => 5,
            CapabilityType::IpcReceive => 6,
            CapabilityType::ProcessSpawn => 7,
            CapabilityType::ProcessKill => 8,
            CapabilityType::FileSystemRead => 9,
            CapabilityType::FileSystemWrite => 10,
            CapabilityType::StorageRawBlock => 11,
            CapabilityType::NetworkSocket => 12,
            CapabilityType::NetworkSend => 13,
            CapabilityType::NetworkReceive => 14,
            CapabilityType::ShmProducer => 15,
            CapabilityType::ShmConsumer => 16,
            CapabilityType::EventPoll => 17,
            CapabilityType::SystemControl => 18,
            CapabilityType::RealtimeClockRead => 24,
        }
    }

    pub const fn from_raw(val: u64) -> Option<Self> {
        match val {
            1 => Some(CapabilityType::SerialOutput),
            2 => Some(CapabilityType::FrameBufferDirect),
            3 => Some(CapabilityType::TimerTickRead),
            4 => Some(CapabilityType::HardwarePortIo),
            5 => Some(CapabilityType::IpcSend),
            6 => Some(CapabilityType::IpcReceive),
            7 => Some(CapabilityType::ProcessSpawn),
            8 => Some(CapabilityType::ProcessKill),
            9 => Some(CapabilityType::FileSystemRead),
            10 => Some(CapabilityType::FileSystemWrite),
            11 => Some(CapabilityType::StorageRawBlock),
            12 => Some(CapabilityType::NetworkSocket),
            13 => Some(CapabilityType::NetworkSend),
            14 => Some(CapabilityType::NetworkReceive),
            15 => Some(CapabilityType::ShmProducer),
            16 => Some(CapabilityType::ShmConsumer),
            17 => Some(CapabilityType::EventPoll),
            18 => Some(CapabilityType::SystemControl),
            24 => Some(CapabilityType::RealtimeClockRead),
            _ => None,
        }
    }
}

/// Bitmask representing granular rights held on a capability.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rights(pub u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EXECUTE: Rights = Rights(1 << 2);
    pub const GRANT: Rights = Rights(1 << 3);
    pub const REVOKE: Rights = Rights(1 << 4);
    pub const INVOKE: Rights = Rights(1 << 5);

    /// Checks if all rights specified in `required` are contained within `self`.
    pub fn contains(self, required: Rights) -> bool {
        (self.0 & required.0) == required.0
    }

    /// Combines two sets of rights.
    pub fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }
}

impl core::ops::BitOr for Rights {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Rights(self.0 | rhs.0)
    }
}

impl fmt::Debug for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        write!(f, "Rights(")?;
        if self.contains(Rights::READ) {
            write!(f, "READ")?;
            first = false;
        }
        if self.contains(Rights::WRITE) {
            if !first { write!(f, " | ")?; }
            write!(f, "WRITE")?;
            first = false;
        }
        if self.contains(Rights::EXECUTE) {
            if !first { write!(f, " | ")?; }
            write!(f, "EXECUTE")?;
            first = false;
        }
        if self.contains(Rights::GRANT) {
            if !first { write!(f, " | ")?; }
            write!(f, "GRANT")?;
            first = false;
        }
        if self.contains(Rights::REVOKE) {
            if !first { write!(f, " | ")?; }
            write!(f, "REVOKE")?;
            first = false;
        }
        if self.contains(Rights::INVOKE) {
            if !first { write!(f, " | ")?; }
            write!(f, "INVOKE")?;
            first = false;
        }
        if first {
            write!(f, "NONE")?;
        }
        write!(f, ")")
    }
}

/// A zero-trust, unforgeable capability token managed exclusively by the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// Unique capability index identifier.
    pub id: CapabilityId,
    /// The target resource category.
    pub cap_type: CapabilityType,
    /// The rights granted on this resource.
    pub rights: Rights,
    /// Object-specific target identifier (e.g. target TaskId, Port, or Address).
    pub object_id: u64,
}

/// An unforgeable, generation-counted opaque capability handle passed across the syscall boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapHandle(pub u64);

impl CapHandle {
    pub const NONE: CapHandle = CapHandle(0);

    #[inline]
    pub fn new(slot: u32, generation: u32) -> Self {
        CapHandle(((generation as u64) << 32) | (slot as u64))
    }

    #[inline]
    pub fn slot(&self) -> usize {
        (self.0 & 0xFFFF_FFFF) as usize
    }

    #[inline]
    pub fn generation(&self) -> u32 {
        (self.0 >> 32) as u32
    }
}

/// A guarded slot inside a process's capability table.
#[derive(Debug)]
pub struct CapSlot {
    pub cap: Option<Capability>,
    pub generation: u32,
    pub access_count: AtomicU64,
    pub last_access_ticks: AtomicU64,
}

impl Clone for CapSlot {
    fn clone(&self) -> Self {
        Self {
            cap: self.cap.clone(),
            generation: self.generation,
            access_count: AtomicU64::new(self.access_count.load(Ordering::Relaxed)),
            last_access_ticks: AtomicU64::new(self.last_access_ticks.load(Ordering::Relaxed)),
        }
    }
}

impl PartialEq for CapSlot {
    fn eq(&self, other: &Self) -> bool {
        self.cap == other.cap
            && self.generation == other.generation
            && self.access_count.load(Ordering::Relaxed)
                == other.access_count.load(Ordering::Relaxed)
            && self.last_access_ticks.load(Ordering::Relaxed)
                == other.last_access_ticks.load(Ordering::Relaxed)
    }
}

impl Eq for CapSlot {}

impl CapSlot {
    pub fn new() -> Self {
        Self {
            cap: None,
            generation: 1,
            access_count: AtomicU64::new(0),
            last_access_ticks: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn record_access(&self) {
        self.access_count.fetch_add(1, Ordering::Relaxed);
        self.last_access_ticks.store(crate::interrupts::get_system_ticks(), Ordering::Relaxed);
    }
}

/// Errors returned by the capability enforcement engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// The requested slot index is empty or out of bounds.
    InvalidSlot,
    /// The capability handle is stale (slot generation changed after revocation).
    StaleHandle,
    /// The capability handle was forged or corrupt.
    ForgedHandle,
    /// The capability has been revoked.
    RevokedHandle,
    /// The capability type does not match the requested operation.
    TypeMismatch,
    /// The capability does not hold the required rights bits.
    InsufficientRights,
    /// The capability object target does not match the requested resource.
    TargetMismatch,
    /// The capability table is full.
    TableFull,
}

/// Base tag for the per-launch-instance capability id space.
///
/// WHY bit 61 and a dedicated constant: three other id spaces must not be
/// reachable from here, or a re-keyed identity could be mistaken for one of them.
/// Permission-minted ids use bit 62 (`PERMISSION_ID_BASE`) and delegated leases use
/// bit 63 (`lease_capability_id`), because `verify_object` classifies a capability
/// as a lease by testing bit 63. App spawner constants and the boot self-test ids
/// (`0x00C0_FFEE_*`, `0x5A11_*`) live in the low 32 bits. Bit 61 is therefore free,
/// and the counter starts well above the constant range.
const INSTANCE_ID_BASE: u64 = 0x2000_0000_0000_0000;

/// Monotonic per-boot counter behind `next_instance_capability_id`.
///
/// WHY `Relaxed`: this only has to be unique, never ordered against other memory.
static NEXT_INSTANCE_CAPABILITY_ID: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x0000_1000);

/// Returns a capability identity that has never been minted before in this boot.
pub fn next_instance_capability_id() -> u64 {
    use core::sync::atomic::Ordering;
    // Saturation rather than wrap: a wrapped counter could reissue a live identity,
    // which is precisely the failure this whole scheme exists to prevent. Practically
    // unreachable (2^52 launches), but the failure mode is silent authority reuse.
    let n = NEXT_INSTANCE_CAPABILITY_ID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.checked_add(1).unwrap_or(v))
        })
        .unwrap_or(0x0000_1000);
    INSTANCE_ID_BASE | n
}

/// Guarded capability table assigned to each process/task.
#[derive(Debug, Clone)]
pub struct CapabilityTable {
    slots: Vec<CapSlot>,
    max_slots: usize,
    owner_task: Option<crate::task::TaskId>,
}

impl CapabilityTable {
    /// Creates a new capability table with a designated capacity.
    pub fn new(max_slots: usize) -> Self {
        let mut slots = Vec::with_capacity(max_slots);
        for _ in 0..max_slots {
            slots.push(CapSlot::new());
        }
        Self { slots, max_slots, owner_task: None }
    }

    /// Creates a new capability table explicitly bound to an owning task.
    pub fn new_for_task(owner: crate::task::TaskId, max_slots: usize) -> Self {
        let mut slots = Vec::with_capacity(max_slots);
        for _ in 0..max_slots {
            slots.push(CapSlot::new());
        }
        Self { slots, max_slots, owner_task: Some(owner) }
    }

    /// Associates an owning task with this capability table.
    pub fn set_owner_task(&mut self, owner: crate::task::TaskId) {
        self.owner_task = Some(owner);
    }

    /// Returns the owning task ID if explicitly bound.
    pub fn owner_task(&self) -> Option<crate::task::TaskId> {
        self.owner_task
    }

    /// Returns the maximum capacity of slots in this table.
    pub fn capacity(&self) -> usize {
        self.max_slots
    }

    /// Returns capability access telemetry (access_count, last_access_ticks) for a slot.
    pub fn access_stats(&self, slot_idx: usize) -> Option<(u64, u64)> {
        self.slots.get(slot_idx).map(|s| {
            (
                s.access_count.load(Ordering::Relaxed),
                s.last_access_ticks.load(Ordering::Relaxed),
            )
        })
    }

    /// Inserts a new capability into the first available slot, returning the generation-counted handle.
    pub fn insert(&mut self, cap: Capability) -> Result<usize, CapError> {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.cap.is_none() {
                slot.cap = Some(cap);
                return Ok(idx);
            }
        }
        Err(CapError::TableFull)
    }

    /// Inserts a capability and returns an unforgeable, generation-counted `CapHandle`.
    pub fn insert_handle(&mut self, cap: Capability) -> Result<CapHandle, CapError> {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.cap.is_none() {
                slot.cap = Some(cap);
                return Ok(CapHandle::new(idx as u32, slot.generation));
            }
        }
        Err(CapError::TableFull)
    }

    /// Retrieves a reference to a capability by slot index.
    ///
    /// WHY the revocation check lives here: every `verify*` method resolves its
    /// capability through this function, so putting the authoritative check at
    /// the single point where a capability leaves the table is what keeps a new
    /// check shape from accidentally consulting only the caller's copy.
    pub fn get(&self, slot_idx: usize) -> Result<&Capability, CapError> {
        let slot = self
            .slots
            .get(slot_idx)
            .ok_or(CapError::InvalidSlot)?;
        let cap = slot.cap.as_ref().ok_or(CapError::InvalidSlot)?;
        if revocation::is_revoked(cap.id) {
            return Err(CapError::RevokedHandle);
        }
        slot.record_access();
        Ok(cap)
    }

    /// Retrieves a capability by its generation-counted `CapHandle`, rejecting stale or forged handles.
    pub fn get_by_handle(&self, handle: CapHandle) -> Result<&Capability, CapError> {
        let slot_idx = handle.slot();
        let slot = self.slots.get(slot_idx).ok_or(CapError::InvalidSlot)?;
        // WHY the generation is compared before the revocation state: a handle
        // minted before the revocation describes a slot that has since changed,
        // and reporting it as stale is the more specific and more useful answer.
        if slot.generation != handle.generation() {
            return Err(CapError::StaleHandle);
        }
        let cap = slot.cap.as_ref().ok_or(CapError::RevokedHandle)?;
        if revocation::is_revoked(cap.id) {
            return Err(CapError::RevokedHandle);
        }
        slot.record_access();
        Ok(cap)
    }

    /// Verifies that the capability at `slot_idx` satisfies the expected type and rights.
    pub fn verify(
        &self,
        slot_idx: usize,
        expected_type: CapabilityType,
        required_rights: Rights,
    ) -> Result<&Capability, CapError> {
        let cap = self.get(slot_idx)?;
        if cap.cap_type != expected_type {
            return Err(CapError::TypeMismatch);
        }
        if !cap.rights.contains(required_rights) {
            return Err(CapError::InsufficientRights);
        }
        if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
            delegation::consume_invocation(
                cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF,
                self.owner_task,
                crate::interrupts::get_system_ticks(),
                required_rights,
            )?;
        }
        Ok(cap)
    }

    /// Inspects whether the capability at `slot_idx` satisfies the expected type and rights
    /// without consuming an invocation quota on delegated leases.
    pub fn inspect_slot(
        &self,
        slot_idx: usize,
        expected_type: CapabilityType,
        required_rights: Rights,
    ) -> Result<&Capability, CapError> {
        let cap = self.get(slot_idx)?;
        if revocation::is_revoked(cap.id) {
            return Err(CapError::RevokedHandle);
        }
        if cap.cap_type != expected_type {
            return Err(CapError::TypeMismatch);
        }
        if !cap.rights.contains(required_rights) {
            return Err(CapError::InsufficientRights);
        }
        if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
            let lease_id = cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF;
            if !delegation::is_lease_valid(
                lease_id,
                self.owner_task,
                crate::interrupts::get_system_ticks(),
                required_rights,
            ) {
                return Err(CapError::InvalidSlot);
            }
        }
        Ok(cap)
    }

    /// Verifies an opaque `CapHandle` against expected type and required rights.
    pub fn verify_handle(
        &self,
        handle: CapHandle,
        expected_type: CapabilityType,
        required_rights: Rights,
    ) -> Result<&Capability, CapError> {
        let cap = self.get_by_handle(handle)?;
        if cap.cap_type != expected_type {
            return Err(CapError::TypeMismatch);
        }
        if !cap.rights.contains(required_rights) {
            return Err(CapError::InsufficientRights);
        }
        if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
            delegation::consume_invocation(
                cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF,
                self.owner_task,
                crate::interrupts::get_system_ticks(),
                required_rights,
            )?;
        }
        Ok(cap)
    }

    /// Checks whether the table contains a matching capability without consuming
    /// delegated invocation quota. This is a pure inspection operation.
    ///
    /// It still refuses a revoked capability: an inspection result is used as an
    /// authorization decision by the network and shared-memory paths, so
    /// answering "yes, you hold it" for a capability whose authority has been
    /// taken back would be a stale-copy answer.
    pub fn has_capability(&self, expected_type: CapabilityType, required_rights: Rights) -> bool {
        let now = crate::interrupts::get_system_ticks();
        self.slots.iter().any(|slot| {
            slot.cap.as_ref().is_some_and(|cap| {
                if cap.cap_type != expected_type || !cap.rights.contains(required_rights) {
                    return false;
                }
                if revocation::is_revoked(cap.id) {
                    return false;
                }
                if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
                    let lease_id = cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF;
                    if !delegation::is_lease_valid(lease_id, self.owner_task, now, required_rights) {
                        return false;
                    }
                }
                true
            })
        })
    }

    /// Authorizes one operation against a matching capability. Delegated
    /// capabilities consume exactly one invocation here; ordinary capabilities
    /// are only inspected. Callers must use this method for security decisions
    /// that represent an actual privileged operation.
    pub fn authorize_capability(
        &self,
        expected_type: CapabilityType,
        required_rights: Rights,
    ) -> bool {
        for slot in &self.slots {
            let Some(cap) = slot.cap.as_ref() else { continue; };
            if cap.cap_type != expected_type || !cap.rights.contains(required_rights) {
                continue;
            }
            if revocation::is_revoked(cap.id) {
                continue;
            }
            if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
                if delegation::consume_invocation(
                    cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF,
                    self.owner_task,
                    crate::interrupts::get_system_ticks(),
                    required_rights,
                ).is_err() {
                    continue;
                }
            }
            slot.record_access();
            return true;
        }
        false
    }

    /// Authorizes a capability against type, rights, and object target. Delegated
    /// capabilities consume one invocation only after every predicate matches.
    pub fn authorize_object(
        &self,
        expected_type: CapabilityType,
        required_rights: Rights,
        expected_object_id: u64,
    ) -> bool {
        for slot in &self.slots {
            let Some(cap) = slot.cap.as_ref() else { continue; };
            if cap.cap_type != expected_type
                || !cap.rights.contains(required_rights)
                || cap.object_id != expected_object_id
            {
                continue;
            }
            if revocation::is_revoked(cap.id) {
                continue;
            }
            if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
                if delegation::consume_invocation(
                    cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF,
                    self.owner_task,
                    crate::interrupts::get_system_ticks(),
                    required_rights,
                ).is_err() {
                    continue;
                }
            }
            slot.record_access();
            return true;
        }
        false
    }

    /// Verifies that the capability in `slot_idx` matches the expected type, rights, and specific object ID.
    pub fn verify_object(
        &self,
        slot_idx: usize,
        expected_type: CapabilityType,
        required_rights: Rights,
        expected_object_id: u64,
    ) -> Result<&Capability, CapError> {
        let cap = self.get(slot_idx)?;
        if cap.cap_type != expected_type {
            return Err(CapError::TypeMismatch);
        }
        if !cap.rights.contains(required_rights) {
            return Err(CapError::InsufficientRights);
        }
        if cap.object_id != expected_object_id {
            return Err(CapError::TargetMismatch);
        }
        if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
            delegation::consume_invocation(
                cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF,
                self.owner_task,
                crate::interrupts::get_system_ticks(),
                required_rights,
            )?;
        }
        Ok(cap)
    }

    /// Revokes and removes the capability at `slot_idx`, incrementing generation to invalidate stale handles.
    ///
    /// WHY clearing the slot is not the enforcement: the capability may exist in
    /// another table, in an IPC message in flight, or in a caller's own copy.
    /// `revocation::revoke` publishes the revocation by identity, so every check
    /// site denies it from then on regardless of how many copies exist. The slot
    /// clearing and generation bump only tidy this table and invalidate handles.
    pub fn revoke(&mut self, slot_idx: usize) -> Result<(), CapError> {
        if slot_idx >= self.slots.len() {
            return Err(CapError::InvalidSlot);
        }
        let Some(cap) = self.slots[slot_idx].cap.take() else {
            return Err(CapError::InvalidSlot);
        };
        self.slots[slot_idx].generation = self.slots[slot_idx].generation.wrapping_add(1);
        revocation::revoke(cap.id);
        Ok(())
    }

    /// Revokes a capability via its `CapHandle`, ensuring use-after-close prevention.
    pub fn revoke_handle(&mut self, handle: CapHandle) -> Result<(), CapError> {
        let slot_idx = handle.slot();
        if slot_idx >= self.slots.len() {
            return Err(CapError::InvalidSlot);
        }
        if self.slots[slot_idx].generation != handle.generation() {
            return Err(CapError::StaleHandle);
        }
        self.revoke(slot_idx)
    }

    /// Revokes a capability by its unique 64-bit CapabilityId.
    ///
    /// Returns whether *this* table held a copy. The revocation is published
    /// either way: whether a particular table happens to hold the capability is
    /// not something the caller should have to get right for the authority to
    /// end.
    pub fn revoke_by_id(&mut self, cap_id: CapabilityId) -> bool {
        let mut held = false;
        // Every slot is cleared, not just the first match: the same capability
        // can legitimately be installed twice, and clearing one copy while
        // leaving the other would leave live authority in a table the caller
        // never inspected.
        for slot in self.slots.iter_mut() {
            if slot.cap.as_ref().map(|cap| cap.id) == Some(cap_id) {
                slot.cap = None;
                slot.generation = slot.generation.wrapping_add(1);
                held = true;
            }
        }
        revocation::revoke(cap_id);
        held
    }

    /// Revokes every capability this table holds.
    ///
    /// Used on the task-exit path. Task identifiers are allocated monotonically
    /// and never reused, so a capability that belonged to an exited task can
    /// never be legitimately held by a later one, and revoking by identity here
    /// also kills any copy of it that escaped into another table.
    pub fn revoke_all(&mut self) -> usize {
        let mut revoked = 0usize;
        for slot in self.slots.iter_mut() {
            if let Some(cap) = slot.cap.take() {
                slot.generation = slot.generation.wrapping_add(1);
                revocation::revoke(cap.id);
                revoked += 1;
            }
        }
        revoked
    }

    /// Re-issues a fresh, never-before-used identity for every occupied slot.
    ///
    /// WHY this exists: app spawners build their capability tables from *constants*
    /// (`CapabilityId(2601)`, `CapabilityId(1701)`, and so on — 101 of them across
    /// 12 spawn functions). `revoke_all` publishes each id into the kernel-wide
    /// revocation set on task exit and nothing ever evicts it, so the second launch
    /// of the same app created a table whose every slot was already revoked. The app
    /// was born with no authorizable capability: `surface_create` was refused,
    /// `surface` came back 0, and the app skipped rendering entirely — which surfaced
    /// as "Recents failed to present a canvas", though Recents was never the fault.
    ///
    /// Revocation-by-identity is only sound while an identity is minted at most once
    /// per boot. Re-keying at the point a table enters the scheduler restores that
    /// invariant for every spawner at once, instead of editing twelve call sites and
    /// hoping the next app added later remembers.
    ///
    /// Type, rights and object_id are preserved: the id is opaque identity, and those
    /// three fields are what actually authorise anything.
    pub fn rekey_all(&mut self) -> usize {
        let mut rekeyed = 0usize;
        for slot in self.slots.iter_mut() {
            if let Some(cap) = slot.cap.as_mut() {
                cap.id = CapabilityId(next_instance_capability_id());
                rekeyed += 1;
            }
        }
        rekeyed
    }

    /// Finds the first slot containing a capability with the requested type and rights.
    ///
    /// Revoked capabilities are skipped so this returns a slot that can actually
    /// authorize, rather than a revoked slot that would make a later `verify*`
    /// fail and mask a valid capability further along the table.
    pub fn find_slot(&self, expected_type: CapabilityType, required_rights: Rights) -> Option<usize> {
        let now = crate::interrupts::get_system_ticks();
        self.slots.iter().enumerate().find_map(|(idx, slot)| {
            slot.cap.as_ref().and_then(|cap| {
                if cap.cap_type != expected_type
                    || !cap.rights.contains(required_rights)
                    || revocation::is_revoked(cap.id)
                {
                    return None;
                }
                if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
                    let lease_id = cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF;
                    if !delegation::is_lease_valid(lease_id, self.owner_task, now, required_rights) {
                        return None;
                    }
                }
                Some(idx)
            })
        })
    }

    /// Finds the first slot containing an unrevoked capability matching type, rights, and optional object ID.
    /// If `expected_object_id == 0`, any matching capability satisfies the search.
    pub fn find_slot_object(
        &self,
        expected_type: CapabilityType,
        required_rights: Rights,
        expected_object_id: u64,
    ) -> Option<usize> {
        let now = crate::interrupts::get_system_ticks();
        self.slots.iter().enumerate().find_map(|(idx, slot)| {
            slot.cap.as_ref().and_then(|cap| {
                if cap.cap_type != expected_type
                    || !cap.rights.contains(required_rights)
                    || (expected_object_id != 0 && cap.object_id != expected_object_id)
                    || revocation::is_revoked(cap.id)
                {
                    return None;
                }
                if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
                    let lease_id = cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF;
                    if !delegation::is_lease_valid(lease_id, self.owner_task, now, required_rights) {
                        return None;
                    }
                }
                slot.record_access();
                Some(idx)
            })
        })
    }

    /// Returns the total number of capabilities in this table that can still
    /// authorize something.
    pub fn count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.cap.as_ref().is_some_and(|cap| !revocation::is_revoked(cap.id)))
            .count()
    }

    /// Validates internal slot invariants. Empty slots retain a generation counter;
    /// active slots must always carry a capability and a non-zero generation.
    pub fn validate_invariants(&self) -> bool {
        self.slots.iter().all(|slot| slot.generation != 0)
    }
}

pub mod delegation;
pub use delegation::DelegatedLease;

pub mod permission;
pub use permission::{ConsentDecision, PermissionManager, PermissionRecord, SensitivePermission, PERMISSION_MANAGER};

pub mod revocation;
pub use revocation::{is_revoked as capability_revoked, revoke as revoke_capability_id};

/// Releases every piece of authority a task is holding.
///
/// Called from the task exit paths, alongside the existing per-subsystem
/// `cleanup_task` calls.
///
/// WHY the registry side matters here: dropping the task's capability table
/// frees the table, but a delegated lease lives in the kernel-wide delegation
/// registry and names the task as its grantee. Without this, that lease stays
/// registered and stays exercisable after the task is gone.
pub fn cleanup_task(task: crate::task::TaskId) {
    let leases = revocation::revoke_grantee_delegations(task);
    delegation::prune(crate::interrupts::get_system_ticks());
    let consents = permission::cleanup_task(task);
    if leases > 0 || consents > 0 {
        crate::serial_println!(
            "[CAPABILITY] Task {} released {} delegated lease(s) and {} permission grant(s) on exit.",
            task.0,
            leases,
            consents,
        );
    }
}

/// Initializes the capability subsystem and runs delegation, permission, and adversarial self-tests.
pub fn init() {
    delegation::init();
    permission::init();
    run_adversarial_tests();
}

/// Executes adversarial verification tests against the capability subsystem.
pub fn run_adversarial_tests() {
    crate::serial_println!("[TEST] Initializing Zero-Trust Capability Security Invariant Suite...");

    let mut table = CapabilityTable::new(16);

    // 1. Insert genuine capability
    let cap = Capability {
        id: CapabilityId(1),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE | Rights::GRANT,
        object_id: 101,
    };
    let handle = table.insert_handle(cap).expect("Failed to insert capability");
    assert!(table.verify_handle(handle, CapabilityType::SerialOutput, Rights::WRITE).is_ok());

    // 2. Adversarial Test 1: Forged Handle Attack (out-of-bounds slot)
    let forged_oob = CapHandle::new(999, 1);
    assert_eq!(table.get_by_handle(forged_oob), Err(CapError::InvalidSlot));
    crate::serial_println!("[TEST] Capability Adversarial: Forged OOB Handle Rejected [PASSED]");

    // 3. Adversarial Test 2: Forged Generation Attack
    let forged_gen = CapHandle::new(handle.slot() as u32, handle.generation() + 10);
    assert_eq!(table.get_by_handle(forged_gen), Err(CapError::StaleHandle));
    crate::serial_println!("[TEST] Capability Adversarial: Forged Generation Token Rejected [PASSED]");

    // 4. Adversarial Test 3: Insufficient Rights Attack
    assert_eq!(
        table.verify_handle(handle, CapabilityType::SerialOutput, Rights::EXECUTE),
        Err(CapError::InsufficientRights)
    );
    crate::serial_println!("[TEST] Capability Adversarial: Rights Escalation Rejected [PASSED]");

    // 5. Adversarial Test 4: Type Mismatch Attack
    assert_eq!(
        table.verify_handle(handle, CapabilityType::HardwarePortIo, Rights::WRITE),
        Err(CapError::TypeMismatch)
    );
    crate::serial_println!("[TEST] Capability Adversarial: Type Confused Invocation Rejected [PASSED]");

    // 6. Adversarial Test 5: Revocation & Use-After-Close Attack
    table.revoke_handle(handle).expect("Failed to revoke capability");
    assert_eq!(table.get_by_handle(handle), Err(CapError::StaleHandle));
    assert_eq!(table.verify(handle.slot(), CapabilityType::SerialOutput, Rights::WRITE), Err(CapError::InvalidSlot));
    crate::serial_println!("[TEST] Capability Adversarial: Use-After-Close Post-Revocation Blocked [PASSED]");

    // Object-target confusion must never grant authority.
    let target_cap = Capability { id: CapabilityId(2), cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 42 };
    let target_slot = table.insert(target_cap).expect("Failed to insert target capability");
    assert_eq!(table.verify_object(target_slot, CapabilityType::IpcSend, Rights::WRITE, 99), Err(CapError::TargetMismatch));
    assert!(table.validate_invariants());
    crate::serial_println!("[TEST] Capability Adversarial: Object Target Confusion Rejected [PASSED]");

    // 7. Adversarial Test 6: Cross-Process Delegated Capability Use Attack
    let parent_cap = Capability {
        id: CapabilityId(10),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE | Rights::GRANT,
        object_id: 42,
    };
    let current_ticks = crate::interrupts::get_system_ticks();
    let lease = delegation::DelegatedLease::new(
        delegation::allocate_lease_id(),
        &parent_cap,
        crate::task::TaskId(1),
        crate::task::TaskId(10),
        Rights::WRITE,
        2,
        1000,
        current_ticks,
    ).expect("Delegated lease creation must succeed");
    delegation::register_lease(lease.clone());
    let derived_cap = lease.to_capability();

    let mut valid_grantee_table = CapabilityTable::new_for_task(crate::task::TaskId(10), 4);
    let valid_slot = valid_grantee_table.insert(derived_cap.clone()).expect("Insert into grantee table");
    assert!(valid_grantee_table.verify_object(valid_slot, CapabilityType::IpcSend, Rights::WRITE, 42).is_ok());

    let mut rogue_task_table = CapabilityTable::new_for_task(crate::task::TaskId(99), 4);
    let rogue_slot = rogue_task_table.insert(derived_cap).expect("Insert into rogue table");
    assert_eq!(
        rogue_task_table.verify_object(rogue_slot, CapabilityType::IpcSend, Rights::WRITE, 42),
        Err(CapError::TargetMismatch)
    );
    crate::serial_println!("[TEST] Capability Adversarial: Cross-Process Use Rejected [PASSED]");

    // 8. Adversarial Test 7: Capability Delegation Escalation Attack
    let read_only_parent = Capability {
        id: CapabilityId(20),
        cap_type: CapabilityType::FileSystemRead,
        rights: Rights::READ | Rights::GRANT,
        object_id: 0,
    };
    let escalation_attempt = delegation::DelegatedLease::new(
        delegation::allocate_lease_id(),
        &read_only_parent,
        crate::task::TaskId(1),
        crate::task::TaskId(2),
        Rights::READ | Rights::WRITE, // Illegally requesting unheld WRITE right
        1,
        100,
        current_ticks,
    );
    assert_eq!(escalation_attempt, Err(CapError::InsufficientRights));
    crate::serial_println!("[TEST] Capability Adversarial: Delegation Escalation Rejected [PASSED]");

    // 9. Adversarial Test 8: Delegated Quota Exhaustion Attack
    let single_use_lease = delegation::DelegatedLease::new(
        delegation::allocate_lease_id(),
        &parent_cap,
        crate::task::TaskId(1),
        crate::task::TaskId(10),
        Rights::WRITE,
        1, // exactly 1 invocation
        500,
        current_ticks,
    ).expect("Single use lease creation");
    delegation::register_lease(single_use_lease.clone());
    let single_cap = single_use_lease.to_capability();
    let mut quota_table = CapabilityTable::new_for_task(crate::task::TaskId(10), 2);
    let q_slot = quota_table.insert(single_cap).expect("Install single use cap");
    assert!(quota_table.verify_object(q_slot, CapabilityType::IpcSend, Rights::WRITE, 42).is_ok());
    assert_eq!(
        quota_table.verify_object(q_slot, CapabilityType::IpcSend, Rights::WRITE, 42),
        Err(CapError::InvalidSlot)
    );
    crate::serial_println!("[TEST] Capability Adversarial: Quota Exhaustion Enforced [PASSED]");

    // 10. Adversarial Test 9: Revocation Must Reach Every Copy Of The Capability
    //
    // Clearing a slot is not enforcement. The same capability installed in a
    // second table is a separate copy of the same authority, and a copy handed
    // out in-band is a third. If any of them still authorizes after the
    // revocation, the process retained authority it was never meant to keep.
    let revokable_id = CapabilityId(0x00C0_FFEE_0001);
    let revokable = Capability {
        id: revokable_id,
        cap_type: CapabilityType::NetworkSend,
        rights: Rights::WRITE,
        object_id: 42,
    };
    let mut primary_table = CapabilityTable::new_for_task(crate::task::TaskId(120), 4);
    let mut copied_table = CapabilityTable::new_for_task(crate::task::TaskId(121), 4);
    let primary_slot = primary_table
        .insert(revokable.clone())
        .expect("insert into primary table");
    let copied_slot = copied_table.insert(revokable).expect("insert into copied table");
    assert!(primary_slot < primary_table.capacity());
    assert!(copied_table.verify_object(copied_slot, CapabilityType::NetworkSend, Rights::WRITE, 42).is_ok());

    assert!(primary_table.revoke_by_id(revokable_id));
    assert_eq!(
        copied_table.verify_object(copied_slot, CapabilityType::NetworkSend, Rights::WRITE, 42),
        Err(CapError::RevokedHandle),
        "A retained copy of a revoked capability must not authorize"
    );
    assert!(!copied_table.authorize_object(CapabilityType::NetworkSend, Rights::WRITE, 42));
    assert!(!copied_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
    assert_eq!(copied_table.find_slot(CapabilityType::NetworkSend, Rights::WRITE), None);
    crate::serial_println!("[TEST] Capability Adversarial: Stale Capability Copy Rejected Post-Revocation [PASSED]");

    // 11. Adversarial Test 10: Revoking A Source Must Kill Its Delegated Leases
    let source_id = CapabilityId(0x00C0_FFEE_0002);
    let source = Capability {
        id: source_id,
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE | Rights::GRANT,
        object_id: 42,
    };
    let mut grantor_table = CapabilityTable::new_for_task(crate::task::TaskId(122), 4);
    grantor_table.insert(source.clone()).expect("insert source capability");

    let derived_lease = DelegatedLease::new(
        delegation::allocate_lease_id(),
        &source,
        crate::task::TaskId(122),
        crate::task::TaskId(123),
        Rights::WRITE,
        4,
        5000,
        crate::interrupts::get_system_ticks(),
    ).expect("delegation must succeed");
    delegation::register_lease(derived_lease.clone());
    let derived_id = derived_lease.to_capability().id;
    let mut grantee_table = CapabilityTable::new_for_task(crate::task::TaskId(123), 4);
    let grantee_slot = grantee_table
        .insert(derived_lease.to_capability())
        .expect("insert derived capability");
    assert!(grantee_table
        .verify_object(grantee_slot, CapabilityType::IpcSend, Rights::WRITE, 42)
        .is_ok());

    assert!(grantor_table.revoke_by_id(source_id));
    assert!(
        revocation::is_revoked(derived_id),
        "a capability derived from a revoked source must itself be revoked"
    );
    assert_eq!(
        grantee_table.verify_object(grantee_slot, CapabilityType::IpcSend, Rights::WRITE, 42),
        Err(CapError::RevokedHandle),
        "A delegated capability must lose its authority when its source is revoked"
    );
    assert!(!delegation::is_lease_active(derived_lease.lease_id, crate::interrupts::get_system_ticks()));
    crate::serial_println!("[TEST] Capability Adversarial: Delegated Authority Dies With Its Source [PASSED]");

    // 12. Adversarial Test 11: A Permission Revocation Ends The Authority Even If
    // The Table Cleanup Never Runs. The consent record alone is not enforcement.
    {
        use crate::capability::permission::{SensitivePermission, PERMISSION_MANAGER};
        let victim = crate::task::TaskId(124);
        let permission = SensitivePermission::CameraCapture;
        let mut pm = PERMISSION_MANAGER.lock();
        let (req_id, _) = pm.request_consent(victim, "adversarial.app", permission, false, 0);
        assert_eq!(pm.resolve_consent(req_id, ConsentDecision::Granted).map(|r| r.decision), Some(ConsentDecision::Granted));

        let mut victim_table = CapabilityTable::new_for_task(victim, 4);
        let minted = victim_table
            .insert(permission.to_capability(victim))
            .expect("insert minted capability");
        let minted_id = permission.capability_id(victim);
        assert!(
            !revocation::is_revoked(minted_id),
            "a freshly granted permission must not already be revoked"
        );
        assert!(
            victim_table
                .verify_object(minted, CapabilityType::HardwarePortIo, Rights::READ, 0xCA00)
                .is_ok(),
            "a freshly minted, unrevoked permission must verify against its own type/object, \
             got {:?} for minted_id {:?}",
            victim_table.verify_object(minted, CapabilityType::HardwarePortIo, Rights::READ, 0xCA00).err(),
            minted_id
        );

        assert!(pm.revoke_permission(victim, permission));
        // Deliberately do not touch victim_table: this is the stale copy.
        assert_eq!(
            victim_table.verify_object(minted, CapabilityType::HardwarePortIo, Rights::READ, 0xCA00),
            Err(CapError::RevokedHandle),
            "A permission revocation must end the authority in the task's own table"
        );
        assert!(!pm.has_permission(victim, permission));
        drop(pm);
    }
    crate::serial_println!("[TEST] Capability Adversarial: Consent Revocation Ends Minted Authority [PASSED]");

    // 13. Adversarial Test 12: A One-Time Grant Ends At First Use
    {
        use crate::capability::permission::{SensitivePermission, PERMISSION_MANAGER};
        let victim = crate::task::TaskId(125);
        let permission = SensitivePermission::RtcClock;
        let mut pm = PERMISSION_MANAGER.lock();
        let (req_id, _) = pm.request_consent(victim, "onetime.app", permission, false, 0);
        pm.resolve_consent(req_id, ConsentDecision::GrantedOnce);
        let mut victim_table = CapabilityTable::new_for_task(victim, 4);
        let minted = victim_table
            .insert(permission.to_capability(victim))
            .expect("insert minted capability");
        let before = victim_table
            .verify_object(minted, CapabilityType::TimerTickRead, Rights::READ, 0x70)
            .is_ok();
        assert!(before);
        assert!(pm.consume_one_time_permission(victim, permission));
        assert_eq!(
            victim_table.verify_object(minted, CapabilityType::TimerTickRead, Rights::READ, 0x70),
            Err(CapError::RevokedHandle),
            "A consumed one-time grant must not leave its capability usable"
        );
        drop(pm);
    }
    crate::serial_println!("[TEST] Capability Adversarial: One-Time Grant Single-Use Enforced [PASSED]");

    // 14. Adversarial Test 13: Task Exit Leaves No Usable Authority Behind
    {
        let exiting = crate::task::TaskId(126);
        let exit_id = CapabilityId(0x00C0_FFEE_0003);
        let mut exiting_table = CapabilityTable::new_for_task(exiting, 4);
        let slot = exiting_table
            .insert(Capability {
                id: exit_id,
                cap_type: CapabilityType::StorageRawBlock,
                rights: Rights::READ | Rights::INVOKE,
                object_id: 0,
            })
            .expect("insert capability");
        assert!(exiting_table
            .verify_object(slot, CapabilityType::StorageRawBlock, Rights::INVOKE, 0)
            .is_ok());

        // The scheduler drops the table; the registry must be told.
        assert_eq!(exiting_table.revoke_all(), 1);
        cleanup_task(exiting);
        assert!(revocation::is_revoked(exit_id));
        assert_eq!(exiting_table.count(), 0);
        assert!(exiting_table.validate_invariants());
        assert!(!exiting_table.authorize_object(CapabilityType::StorageRawBlock, Rights::INVOKE, 0));
    }
    crate::serial_println!("[TEST] Capability Adversarial: Task Exit Releases All Authority [PASSED]");

    // 15. Adversarial Test 14: Per-Capability Access Accounting Telemetry
    {
        let audit_task = crate::task::TaskId(127);
        let mut audit_table = CapabilityTable::new_for_task(audit_task, 2);
        let slot = audit_table
            .insert(Capability {
                id: CapabilityId(0x00C0_FFEE_0004),
                cap_type: CapabilityType::RealtimeClockRead,
                rights: Rights::READ,
                object_id: 0,
            })
            .expect("insert capability");

        let stats0 = audit_table.access_stats(slot).expect("stats slot 0");
        assert_eq!(stats0.0, 0, "Initial access_count must be 0");

        let _ = audit_table.get(slot);
        let stats1 = audit_table.access_stats(slot).expect("stats slot 0");
        assert_eq!(stats1.0, 1, "access_count must increment after get()");

        let _ = audit_table.verify(slot, CapabilityType::RealtimeClockRead, Rights::READ);
        let stats2 = audit_table.access_stats(slot).expect("stats slot 0");
        assert_eq!(stats2.0, 2, "access_count must increment after verify()");
    }
    crate::serial_println!("[TEST] Capability Adversarial: Per-Capability Access Accounting Verified [PASSED]");

    // 16. Adversarial Test 15: Deterministic Capability Slot Discovery
    {
        let lookup_task = crate::task::TaskId(128);
        let mut table = CapabilityTable::new_for_task(lookup_task, 4);
        let slot_fs = table.insert(Capability {
            id: CapabilityId(0x00C0_FFEE_0010),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 0,
        }).expect("insert fs");
        let slot_ipc = table.insert(Capability {
            id: CapabilityId(0x00C0_FFEE_0011),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: 0x5555,
        }).expect("insert ipc");

        assert_eq!(table.find_slot_object(CapabilityType::FileSystemRead, Rights::NONE, 0), Some(slot_fs));
        assert_eq!(table.find_slot_object(CapabilityType::IpcSend, Rights::NONE, 0x5555), Some(slot_ipc));
        assert_eq!(table.find_slot_object(CapabilityType::IpcSend, Rights::NONE, 0x9999), None);
        assert_eq!(table.find_slot_object(CapabilityType::NetworkSocket, Rights::NONE, 0), None);
    }
    crate::serial_println!("[TEST] Capability Adversarial: Deterministic Slot Discovery Verified [PASSED]");

    crate::serial_println!("[TEST] Zero-Trust Capability Invariants: 100% Attested & Enforced.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_type_raw_encoding_roundtrips() {
        for val in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 24] {
            let cap_type = CapabilityType::from_raw(val).expect("valid capability type");
            assert_eq!(cap_type.to_raw(), val);
        }
        assert_eq!(CapabilityType::from_raw(0), None);
        assert_eq!(CapabilityType::from_raw(999), None);
    }

    #[test]
    fn find_slot_object_discovers_unrevoked_capabilities_and_records_telemetry() {
        let task_id = crate::task::TaskId(42);
        let mut table = CapabilityTable::new_for_task(task_id, 4);
        let s0 = table.insert(Capability {
            id: CapabilityId(0x1000),
            cap_type: CapabilityType::SerialOutput,
            rights: Rights::WRITE,
            object_id: 0,
        }).unwrap();
        let s1 = table.insert(Capability {
            id: CapabilityId(0x1001),
            cap_type: CapabilityType::FileSystemRead,
            rights: Rights::READ,
            object_id: 0x1234,
        }).unwrap();

        assert_eq!(table.find_slot_object(CapabilityType::SerialOutput, Rights::NONE, 0), Some(s0));
        assert_eq!(table.find_slot_object(CapabilityType::FileSystemRead, Rights::NONE, 0x1234), Some(s1));
        assert_eq!(table.find_slot_object(CapabilityType::FileSystemRead, Rights::NONE, 0), Some(s1)); // 0 matches any object
        assert_eq!(table.find_slot_object(CapabilityType::FileSystemRead, Rights::NONE, 0x9999), None); // Mismatched object
        assert_eq!(table.find_slot_object(CapabilityType::NetworkSocket, Rights::NONE, 0), None);

        // Access telemetry incremented on discovery
        let (count, _) = table.access_stats(s0).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn delegated_lease_has_capability_and_quota_enforcement() {
        let task_id = crate::task::TaskId(10);
        let parent_cap = Capability {
            id: CapabilityId(500),
            cap_type: CapabilityType::NetworkSend,
            rights: Rights::WRITE.union(Rights::GRANT),
            object_id: 0,
        };
        let current_ticks = crate::interrupts::get_system_ticks();

        // 1. Valid lease with 2 invocations for 100 ticks
        let lease = delegation::DelegatedLease::new(
            5001,
            &parent_cap,
            crate::task::TaskId(1),
            task_id,
            Rights::WRITE,
            2,
            100,
            current_ticks,
        ).expect("create lease");
        delegation::register_lease(lease.clone());

        let mut table = CapabilityTable::new_for_task(task_id, 2);
        table.insert(lease.to_capability()).expect("insert lease");

        // Inspecting capability returns true
        assert!(table.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        // Wrong rights or type returns false
        assert!(!table.has_capability(CapabilityType::NetworkSend, Rights::READ));
        assert!(!table.has_capability(CapabilityType::NetworkReceive, Rights::WRITE));

        // Unauthorized caller task returns false
        let mut rogue_table = CapabilityTable::new_for_task(crate::task::TaskId(99), 2);
        rogue_table.insert(lease.to_capability()).expect("insert rogue");
        assert!(!rogue_table.has_capability(CapabilityType::NetworkSend, Rights::WRITE));

        // Authorize invocation 1 consumes 1 quota
        assert!(table.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE));
        // Inspection still succeeds while quota / lease is active
        assert!(table.has_capability(CapabilityType::NetworkSend, Rights::WRITE));

        // Authorize invocation 2 consumes remaining quota
        assert!(table.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE));

        // Quota is exhausted (0 remaining), so further authorization must fail
        assert!(!table.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE));

        // Revoking lease makes has_capability fail
        let lease2 = delegation::DelegatedLease::new(
            5002,
            &parent_cap,
            crate::task::TaskId(1),
            task_id,
            Rights::WRITE,
            5,
            100,
            current_ticks,
        ).expect("create lease 2");
        delegation::register_lease(lease2.clone());
        let mut table2 = CapabilityTable::new_for_task(task_id, 2);
        table2.insert(lease2.to_capability()).expect("insert lease 2");
        assert!(table2.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        delegation::revoke_lease(5002);
        assert!(!table2.has_capability(CapabilityType::NetworkSend, Rights::WRITE));
        assert!(!table2.authorize_capability(CapabilityType::NetworkSend, Rights::WRITE));
    }
}


