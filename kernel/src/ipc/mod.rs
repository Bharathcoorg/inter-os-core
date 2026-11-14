// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS Capability-Guarded Inter-Process Communication (IPC)
//!
//! Provides synchronous and asynchronous zero-copy message passing between tasks.
//! Communication channels are strictly guarded by capabilities: a task cannot
//! transmit messages to another task without possessing an `IpcSend` capability
//! specifically targeted at the recipient's `TaskId`.
//!
//! Two rules hold for every delivery path in this module:
//! 1. Userspace-originated sends verify an `IpcSend` capability whose
//!    `object_id` equals the destination `TaskId` (`verify_object`).
//! 2. Every write funnels through `writable_mailbox`, which rejects a
//!    destination whose endpoint has been revoked.

pub mod secure_channel;
pub mod share;
pub mod shm_pipe;

pub use secure_channel::{SecureChannelManager, SecureIpcError, SecureSession, SignedIpcFrame, SECURE_IPC};
pub use share::{ShareSheetManager, ShareTarget, ShareTransaction, SHARE_SHEET_MANAGER, IPC_MSG_SHARE};
pub use shm_pipe::{ShmError, ShmPipeManager, ShmRingBuffer, ShmSlot, DmaDescriptor, SHM_PIPE_MANAGER};

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::{CapError, Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::task::TaskId;

/// Maximum payload size for high-speed microkernel IPC messages.
pub const IPC_MAX_PAYLOAD: usize = 64;

/// Capacity of each task's incoming message mailbox.
pub const MAILBOX_CAPACITY: usize = 16;

static NEXT_ENDPOINT_ID: AtomicU64 = AtomicU64::new(1);

/// Unique IPC endpoint identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointId(pub u64);

/// Authenticated provenance of an IPC message verified by the microkernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageProvenance {
    /// Originated from the kernel supervisor (TaskId(0)).
    Kernel,
    /// Originated from an authenticated system service with verified ServiceId.
    SystemService(u8),
    /// Originated from an isolated Ring 3 application task.
    UserTask(TaskId),
}

impl MessageProvenance {
    /// Derives authentic provenance for a sender TaskId using kernel service registry.
    pub fn derive(sender: TaskId) -> Self {
        if sender == TaskId(0) {
            MessageProvenance::Kernel
        } else {
            let mut matched_service = None;
            for sid_code in 0..16 {
                if let Some(sid) = crate::services::ServiceId::from_u8(sid_code) {
                    if crate::services::task_id(sid) == Some(sender) {
                        matched_service = Some(sid_code);
                        break;
                    }
                }
            }
            if let Some(code) = matched_service {
                MessageProvenance::SystemService(code)
            } else {
                MessageProvenance::UserTask(sender)
            }
        }
    }
}

/// A discrete IPC message exchanged between two tasks.
#[derive(Debug, Clone)]
pub struct IpcMessage {
    /// Authenticated identity of the transmitting task.
    pub sender: TaskId,
    /// Kernel-verified message provenance.
    pub provenance: MessageProvenance,
    /// Protocol or application-defined message type code.
    pub msg_type: u32,
    /// Optional capability transferred in-band with the message.
    pub transferred_cap: Option<crate::capability::Capability>,
    /// Optional shared-memory descriptor transferred with the message.
    pub shm_descriptor: Option<u32>,
    /// Fixed-size zero-copy inline payload buffer.
    pub payload: [u8; IPC_MAX_PAYLOAD],
    /// Length of active data in the payload buffer.
    pub len: usize,
}

impl IpcMessage {
    /// Constructs a new IPC message from a byte slice with kernel-verified provenance.
    pub fn new(sender: TaskId, msg_type: u32, data: &[u8]) -> Self {
        let len = data.len().min(IPC_MAX_PAYLOAD);
        let mut payload = [0u8; IPC_MAX_PAYLOAD];
        payload[..len].copy_from_slice(&data[..len]);
        let provenance = MessageProvenance::derive(sender);
        Self {
            sender,
            provenance,
            msg_type,
            transferred_cap: None,
            shm_descriptor: None,
            payload,
            len,
        }
    }

    /// Constructs an IPC message with an in-band transferred capability.
    pub fn with_capability(
        sender: TaskId,
        msg_type: u32,
        data: &[u8],
        cap: crate::capability::Capability,
    ) -> Self {
        let mut msg = Self::new(sender, msg_type, data);
        msg.transferred_cap = Some(cap);
        msg
    }

    /// Constructs an IPC message with an associated shared-memory pipe descriptor.
    pub fn with_shm(sender: TaskId, msg_type: u32, data: &[u8], shm_desc: u32) -> Self {
        let mut msg = Self::new(sender, msg_type, data);
        msg.shm_descriptor = Some(shm_desc);
        msg
    }
}

/// Errors returned by the IPC subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Capability validation failed (unauthorized communication attempt).
    CapabilityDenied(CapError),
    /// The target task does not exist or has terminated.
    TargetNotFound,
    /// The recipient's mailbox is full (backpressure signal).
    MailboxFull,
    /// No messages available in the mailbox.
    MailboxEmpty,
    /// The communication endpoint has been revoked or closed.
    EndpointRevoked,
}

impl From<CapError> for IpcError {
    fn from(err: CapError) -> Self {
        IpcError::CapabilityDenied(err)
    }
}

/// An IPC communication endpoint object.
#[derive(Debug, Clone)]
pub struct IpcEndpoint {
    pub id: EndpointId,
    pub owner: TaskId,
    pub is_revoked: bool,
}

/// A task's incoming message queue.
struct TaskMailbox {
    task_id: TaskId,
    queue: VecDeque<IpcMessage>,
}

/// Global system IPC router managing mailboxes and endpoints for all tasks.
pub struct IpcRouter {
    mailboxes: Vec<TaskMailbox>,
    endpoints: Vec<IpcEndpoint>,
}

impl IpcRouter {
    pub const fn new() -> Self {
        Self {
            mailboxes: Vec::new(),
            endpoints: Vec::new(),
        }
    }

    /// Registers a mailbox and a live inbound endpoint for a newly spawned task.
    ///
    /// The endpoint MUST be created here rather than lazily. `endpoints` is the
    /// authorization record for "this task's inbox may be written at all"; a
    /// table that is never populated makes every ownership and revocation check
    /// below vacuously succeed, which is how a revoked or unregistered target
    /// becomes indistinguishable from a permitted one.
    pub fn register_task(&mut self, task_id: TaskId) -> bool {
        if self.mailboxes.iter().any(|m| m.task_id == task_id) {
            return true;
        }
        if task_id != TaskId(0) {
            if let Some(process) = crate::task::process::process_for_task(task_id) {
                if crate::task::process::reserve_ipc_endpoint(process).is_err() {
                    return false;
                }
            }
        }
        if !self.mailboxes.iter().any(|m| m.task_id == task_id) {
            self.mailboxes.push(TaskMailbox {
                task_id,
                queue: VecDeque::with_capacity(MAILBOX_CAPACITY),
            });
        }
        if !self.endpoints.iter().any(|e| e.owner == task_id) {
            let id = EndpointId(NEXT_ENDPOINT_ID.fetch_add(1, Ordering::Relaxed));
            self.endpoints.push(IpcEndpoint { id, owner: task_id, is_revoked: false });
        }
        true
    }

    /// Revokes the inbound endpoint owned by `task_id` without destroying state.
    ///
    /// This is the real revocation path: a revoked endpoint rejects every
    /// subsequent delivery even while the mailbox is still present, so a peer
    /// that lost authorization cannot keep injecting into a live task.
    pub fn revoke_endpoint(&mut self, task_id: TaskId) {
        for ep in self.endpoints.iter_mut() {
            if ep.owner == task_id {
                ep.is_revoked = true;
            }
        }
    }

    /// Returns a mutable mailbox only when the destination is registered and its
    /// endpoint has not been revoked.
    ///
    /// WHY the endpoint check belongs here and not in each caller: every
    /// delivery path funnels through this lookup, so centralizing it makes
    /// "revoked" mean the same thing for send, direct send, and system delivery.
    fn writable_mailbox(&mut self, dest_task: TaskId) -> Result<&mut TaskMailbox, IpcError> {
        if self.endpoints.iter().any(|e| e.owner == dest_task && e.is_revoked) {
            return Err(IpcError::EndpointRevoked);
        }
        self.mailboxes
            .iter_mut()
            .find(|m| m.task_id == dest_task)
            .ok_or(IpcError::TargetNotFound)
    }

    /// Cleans up all IPC resources when a process/task dies or exits.
    pub fn cleanup_task(&mut self, task_id: TaskId) {
        // Revoke first, then drop the mailbox. Revocation is recorded before the
        // mailbox disappears so the state transition is real rather than a
        // no-op marker that is deleted in the same breath.
        self.revoke_endpoint(task_id);

        // 1. Remove task's mailbox and discard pending messages
        self.mailboxes.retain(|m| m.task_id != task_id);

        // 2. Drop the endpoint record; the mailbox no longer exists, so a
        //    recreated task id gets a fresh, unrevoked endpoint.
        self.endpoints.retain(|ep| ep.owner != task_id);
        if task_id != TaskId(0) {
            if let Some(process) = crate::task::process::process_for_task(task_id) {
                crate::task::process::release_ipc_endpoint(process);
            }
        }
    }

    /// Sends a message from `sender` to `dest_task`, strictly verifying that the sender
    /// holds an `IpcSend` capability targeted at `dest_task`.
    pub fn send(
        &mut self,
        sender: TaskId,
        sender_caps: &CapabilityTable,
        cap_slot: usize,
        dest_task: TaskId,
        msg_type: u32,
        data: &[u8],
    ) -> Result<(), IpcError> {
        // Enforce capability invariant: Sender must present an IpcSend token targeted at dest_task.0
        sender_caps
            .verify_object(cap_slot, CapabilityType::IpcSend, Rights::WRITE, dest_task.0)?;

        let mailbox = self.writable_mailbox(dest_task)?;

        if mailbox.queue.len() >= MAILBOX_CAPACITY {
            return Err(IpcError::MailboxFull);
        }

        // Background applications receive a bounded IPC budget so a hidden
        // process cannot keep the system busy by continuously waking peers.
        if let Some(_) = crate::task::process::process_for_task(sender) {
            crate::task::process::consume_background_ipc(sender)
                .map_err(|_| IpcError::MailboxFull)?;
        }

        let msg = IpcMessage::new(sender, msg_type, data);
        mailbox.queue.push_back(msg);
        crate::event::post_event(dest_task, crate::event::EVENT_IPC_MSG);
        Ok(())
    }

    /// Delivers a system-mediated message to `dest` on behalf of `sender`.
    ///
    /// This is a privileged kernel path (share-sheet bus, capability transfer),
    /// NOT a bypass for userspace senders. It still requires the caller's
    /// `IpcSend` capability to be bound to `dest`: the share sheet resolves a
    /// caller-chosen `target_id_str` to a task, so without object scoping one
    /// `IpcSend` lets a caller inject an attacker-chosen kind and payload into
    /// any registered task's mailbox.
    pub fn deliver_system_message(
        &mut self,
        sender: TaskId,
        sender_caps: &CapabilityTable,
        cap_slot: usize,
        dest: TaskId,
        mut msg: IpcMessage,
    ) -> Result<(), IpcError> {
        sender_caps.verify_object(cap_slot, CapabilityType::IpcSend, Rights::WRITE, dest.0)?;

        let mailbox = self.writable_mailbox(dest)?;
        if mailbox.queue.len() >= MAILBOX_CAPACITY {
            return Err(IpcError::MailboxFull);
        }

        // Enforce kernel provenance: the sender metadata cannot be spoofed by caller
        msg.sender = sender;
        msg.provenance = MessageProvenance::derive(sender);

        mailbox.queue.push_back(msg);
        crate::event::post_event(dest, crate::event::EVENT_IPC_MSG);
        Ok(())
    }

    /// Receives a message for `receiver`, verifying that the receiver holds an `IpcReceive` capability.
    pub fn receive(
        &mut self,
        receiver: TaskId,
        receiver_caps: &CapabilityTable,
        cap_slot: usize,
    ) -> Result<Option<IpcMessage>, IpcError> {
        // Enforce capability invariant: Receiver must present an IpcReceive token
        receiver_caps
            .verify(cap_slot, CapabilityType::IpcReceive, Rights::READ)?;

        // A revoked endpoint blocks reads too: a peer whose channel was revoked
        // must not be able to drain whatever the endpoint already buffered.
        if self.endpoints.iter().any(|e| e.owner == receiver && e.is_revoked) {
            return Err(IpcError::EndpointRevoked);
        }

        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.task_id == receiver)
            .ok_or(IpcError::TargetNotFound)?;

        Ok(mailbox.queue.pop_front())
    }

    /// Sends a pre-authorized message directly to the target mailbox.
    ///
    /// This is the kernel-internal delivery path (input routing, focus events,
    /// already-checked syscall dispatch). It deliberately performs NO capability
    /// check: it exists only for callers that are not userspace senders, namely
    /// the kernel itself (`TaskId(0)`) and the syscall layer after it has
    /// already run `verify_object(IpcSend, dest)`. The object-scoped check is
    /// enforced at those call sites, not here, so that a userspace `IpcSend`
    /// can never be spent against an arbitrary destination here.
    pub fn send_direct(
        &mut self,
        sender: TaskId,
        dest_task: TaskId,
        msg_type: u32,
        data: &[u8],
    ) -> Result<(), IpcError> {
        let mailbox = self.writable_mailbox(dest_task)?;
        if mailbox.queue.len() >= MAILBOX_CAPACITY {
            return Err(IpcError::MailboxFull);
        }

        let msg = IpcMessage::new(sender, msg_type, data);
        mailbox.queue.push_back(msg);
        crate::event::post_event(dest_task, crate::event::EVENT_IPC_MSG);
        Ok(())
    }

    /// Receives a message directly from the target mailbox for a pre-verified receiver.
    pub fn receive_direct(&mut self, receiver: TaskId) -> Result<Option<IpcMessage>, IpcError> {
        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.task_id == receiver)
            .ok_or(IpcError::TargetNotFound)?;

        Ok(mailbox.queue.pop_front())
    }

    /// Returns the number of registered task mailboxes.
    pub fn mailbox_count(&self) -> usize {
        self.mailboxes.len()
    }

    /// Returns the number of active IPC endpoints.
    pub fn endpoint_count(&self) -> usize {
        self.endpoints.len()
    }
}

/// Global system-wide IPC message router.
pub static IPC_ROUTER: Mutex<IpcRouter> = Mutex::new(IpcRouter::new());

/// Cleans up IPC mailboxes and channels upon task termination.
pub fn cleanup_task(task_id: TaskId) {
    IPC_ROUTER.lock().cleanup_task(task_id);
    SECURE_IPC.lock().cleanup_task(task_id);
    SHARE_SHEET_MANAGER.lock().cleanup_task(task_id);
    SHM_PIPE_MANAGER.lock().cleanup_task(task_id);
}

/// Initializes the IPC subsystem, secure channels, share bus, and zero-copy SHM pipes.
pub fn init() {
    secure_channel::init();
    share::init();
    shm_pipe::init();
    run_ipc_adversarial_tests();
}

/// Executes adversarial verification tests against the IPC router and endpoints.
pub fn run_ipc_adversarial_tests() {
    crate::serial_println!("[TEST] Initializing Zero-Trust Microkernel IPC Invariant Suite...");

    let mut router = IpcRouter::new();
    let sender_id = TaskId(101);
    let target_id = TaskId(202);

    router.register_task(target_id);

    // 1. Backpressure & Queue Limits Test
    for i in 0..MAILBOX_CAPACITY {
        assert!(router.send_direct(sender_id, target_id, i as u32, b"DATA").is_ok());
    }
    // 17th message must fail due to backpressure / mailbox full
    assert_eq!(
        router.send_direct(sender_id, target_id, 999, b"OVERFLOW"),
        Err(IpcError::MailboxFull)
    );
    crate::serial_println!("[TEST] IPC Invariant: Bounded FIFO Queue Backpressure Enforced [PASSED]");

    // 2. Capability Transfer In-Band Test
    // The sender is granted an IpcSend bound ONLY to target_id. A second
    // capability bound to a different object stands in for "some other peer",
    // which must not be usable to reach target_id.
    let mut sender_caps = CapabilityTable::new(2);
    sender_caps
        .insert(Capability {
            id: CapabilityId(54),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: target_id.0,
        })
        .expect("sender capability table must have room");
    let other_peer_slot = sender_caps
        .insert(Capability {
            id: CapabilityId(56),
            cap_type: CapabilityType::IpcSend,
            rights: Rights::WRITE,
            object_id: target_id.0 ^ 0xFFFF,
        })
        .expect("second sender capability slot");

    let transferred = Capability {
        id: CapabilityId(55),
        cap_type: CapabilityType::SerialOutput,
        rights: Rights::WRITE,
        object_id: 1,
    };
    let cap_msg = IpcMessage::with_capability(sender_id, 10, b"CAP_TRANSFER", transferred);
    // Drain one message to make space
    assert!(router.receive_direct(target_id).unwrap().is_some());
    let bound_slot = 0;
    assert!(router
        .deliver_system_message(sender_id, &sender_caps, bound_slot, target_id, cap_msg)
        .is_ok());

    // An IpcSend bound to a different object must NOT reach target_id.
    let forged_msg = IpcMessage::new(sender_id, 0xDEAD, b"FORGED");
    assert_eq!(
        router.deliver_system_message(sender_id, &sender_caps, other_peer_slot, target_id, forged_msg),
        Err(IpcError::CapabilityDenied(CapError::TargetMismatch))
    );
    crate::serial_println!("[TEST] IPC Invariant: System Delivery Bound to Destination Object [PASSED]");

    // 3. Endpoint Revocation Test
    // Revoking the endpoint must block delivery while the mailbox still exists,
    // otherwise revocation is indistinguishable from "never registered".
    router.revoke_endpoint(target_id);
    let revoked_msg = IpcMessage::new(sender_id, 0xBEEF, b"AFTER_REVOKE");
    assert_eq!(
        router.deliver_system_message(sender_id, &sender_caps, bound_slot, target_id, revoked_msg),
        Err(IpcError::EndpointRevoked)
    );
    crate::serial_println!("[TEST] IPC Invariant: Endpoint Revocation Blocks Live-Task Delivery [PASSED]");

    // 4. Process Death Cleanup Test
    router.cleanup_task(target_id);
    assert_eq!(
        router.send_direct(sender_id, target_id, 1, b"POST_MORTEM"),
        Err(IpcError::TargetNotFound)
    );
    crate::serial_println!("[TEST] IPC Invariant: Process Death Mailbox & Endpoint Cleanup Enforced [PASSED]");

    // 5. Authenticated Sender Provenance Test (Anti-Spoofing)
    // A caller attempting to pass a preconstructed message with a forged sender ID (e.g. TaskId(0))
    // must have the message sender strictly overwritten by the kernel with the verified caller ID.
    router.register_task(target_id);
    let forged_sender_msg = IpcMessage::new(TaskId(0), 0x5555, b"FORGED_SENDER_ATTEMPT");
    assert_eq!(forged_sender_msg.sender, TaskId(0));
    assert_eq!(forged_sender_msg.provenance, MessageProvenance::Kernel);

    assert!(router
        .deliver_system_message(sender_id, &sender_caps, bound_slot, target_id, forged_sender_msg)
        .is_ok());

    let received = router.receive_direct(target_id).unwrap().expect("must receive message");
    assert_eq!(received.sender, sender_id, "Kernel must overwrite forged sender ID with authenticated sender");
    assert_eq!(received.provenance, MessageProvenance::UserTask(sender_id), "Kernel must stamp authentic UserTask provenance");
    crate::serial_println!("[TEST] IPC Invariant: Authenticated Sender Provenance Anti-Spoofing Enforced [PASSED]");

    crate::serial_println!("[TEST] Zero-Trust Microkernel IPC: 100% Attested & Enforced.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_router_adversarial_suite() {
        run_ipc_adversarial_tests();
    }
}


