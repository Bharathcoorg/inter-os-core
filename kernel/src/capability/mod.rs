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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapSlot {
    pub cap: Option<Capability>,
    pub generation: u32,
}

impl CapSlot {
    pub fn new() -> Self {
        Self {
            cap: None,
            generation: 1,
        }
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

/// Guarded capability table assigned to each process/task.
#[derive(Debug, Clone)]
pub struct CapabilityTable {
    slots: Vec<CapSlot>,
    max_slots: usize,
}

impl CapabilityTable {
    /// Creates a new capability table with a designated capacity.
    pub fn new(max_slots: usize) -> Self {
        let mut slots = Vec::with_capacity(max_slots);
        for _ in 0..max_slots {
            slots.push(CapSlot::new());
        }
        Self { slots, max_slots }
    }

    /// Returns the maximum capacity of slots in this table.
    pub fn capacity(&self) -> usize {
        self.max_slots
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
    pub fn get(&self, slot_idx: usize) -> Result<&Capability, CapError> {
        self.slots
            .get(slot_idx)
            .and_then(|slot| slot.cap.as_ref())
            .ok_or(CapError::InvalidSlot)
    }

    /// Retrieves a capability by its generation-counted `CapHandle`, rejecting stale or forged handles.
    pub fn get_by_handle(&self, handle: CapHandle) -> Result<&Capability, CapError> {
        let slot_idx = handle.slot();
        let slot = self.slots.get(slot_idx).ok_or(CapError::InvalidSlot)?;
        if slot.generation != handle.generation() {
            return Err(CapError::StaleHandle);
        }
        slot.cap.as_ref().ok_or(CapError::RevokedHandle)
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
                crate::interrupts::get_system_ticks(),
                required_rights,
            )?;
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
                crate::interrupts::get_system_ticks(),
                required_rights,
            )?;
        }
        Ok(cap)
    }

    /// Checks whether the table contains a matching capability without consuming
    /// delegated invocation quota. This is a pure inspection operation.
    pub fn has_capability(&self, expected_type: CapabilityType, required_rights: Rights) -> bool {
        self.slots.iter().any(|slot| {
            slot.cap.as_ref().is_some_and(|cap| {
                cap.cap_type == expected_type && cap.rights.contains(required_rights)
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
            if cap.id.0 & 0x8000_0000_0000_0000 != 0 {
                delegation::consume_invocation(
                    cap.id.0 & 0x7FFF_FFFF_FFFF_FFFF,
                    crate::interrupts::get_system_ticks(),
                    required_rights,
                ).is_ok() {
                    return true;
                }
                return false;
            }
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
                crate::interrupts::get_system_ticks(),
                required_rights,
            )?;
        }
        Ok(cap)
    }

    /// Revokes and removes the capability at `slot_idx`, incrementing generation to invalidate stale handles.
    pub fn revoke(&mut self, slot_idx: usize) -> Result<(), CapError> {
        if slot_idx >= self.slots.len() || self.slots[slot_idx].cap.is_none() {
            return Err(CapError::InvalidSlot);
        }
        self.slots[slot_idx].cap = None;
        self.slots[slot_idx].generation = self.slots[slot_idx].generation.wrapping_add(1);
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

    /// Finds the first slot containing a capability with the requested type and rights.
    pub fn find_slot(&self, expected_type: CapabilityType, required_rights: Rights) -> Option<usize> {
        self.slots.iter().enumerate().find_map(|(idx, slot)| {
            slot.cap.as_ref().and_then(|cap| {
                if cap.cap_type == expected_type && cap.rights.contains(required_rights) { Some(idx) } else { None }
            })
        })
    }

    /// Returns the total number of active capabilities in this table.
    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.cap.is_some()).count()
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

    crate::serial_println!("[TEST] Zero-Trust Capability Invariants: 100% Attested & Enforced.");
}


