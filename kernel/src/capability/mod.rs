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

/// Errors returned by the capability enforcement engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// The requested slot index is empty or out of bounds.
    InvalidSlot,
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
    slots: Vec<Option<Capability>>,
    max_slots: usize,
}

impl CapabilityTable {
    /// Creates a new capability table with a designated capacity.
    pub fn new(max_slots: usize) -> Self {
        let mut slots = Vec::with_capacity(max_slots);
        for _ in 0..max_slots {
            slots.push(None);
        }
        Self { slots, max_slots }
    }

    /// Returns the maximum capacity of slots in this table.
    pub fn capacity(&self) -> usize {
        self.max_slots
    }

    /// Inserts a new capability into the first available slot, returning the slot index.
    pub fn insert(&mut self, cap: Capability) -> Result<usize, CapError> {
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(cap);
                return Ok(idx);
            }
        }
        Err(CapError::TableFull)
    }

    /// Retrieves a reference to a capability by slot index.
    pub fn get(&self, slot_idx: usize) -> Result<&Capability, CapError> {
        self.slots
            .get(slot_idx)
            .and_then(|opt| opt.as_ref())
            .ok_or(CapError::InvalidSlot)
    }

    /// Verifies that the capability in `slot_idx` satisfies the expected type and rights.
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
        Ok(cap)
    }

    /// Checks if this capability table contains at least one capability of the expected type and rights.
    pub fn has_capability(&self, expected_type: CapabilityType, required_rights: Rights) -> bool {
        for slot in &self.slots {
            if let Some(ref cap) = slot {
                if cap.cap_type == expected_type && cap.rights.contains(required_rights) {
                    return true;
                }
            }
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
        let cap = self.verify(slot_idx, expected_type, required_rights)?;
        if cap.object_id != expected_object_id {
            return Err(CapError::TargetMismatch);
        }
        Ok(cap)
    }

    /// Revokes and removes the capability at `slot_idx`.
    pub fn revoke(&mut self, slot_idx: usize) -> Result<(), CapError> {
        if slot_idx >= self.slots.len() || self.slots[slot_idx].is_none() {
            return Err(CapError::InvalidSlot);
        }
        self.slots[slot_idx] = None;
        Ok(())
    }

    /// Returns the total number of active capabilities in this table.
    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

pub mod delegation;
pub use delegation::DelegatedLease;

pub mod permission;
pub use permission::{ConsentDecision, PermissionManager, PermissionRecord, SensitivePermission, PERMISSION_MANAGER};

/// Initializes the capability subsystem and runs delegation and permission self-tests.
pub fn init() {
    delegation::init();
    permission::init();
}

