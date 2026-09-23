//! Inter OS Capability-Guarded Inter-Process Communication (IPC)
//!
//! Provides synchronous and asynchronous zero-copy message passing between tasks.
//! Communication channels are strictly guarded by capabilities: a task cannot
//! transmit messages to another task without possessing an `IpcSend` capability
//! specifically targeted at the recipient's `TaskId`.

pub mod secure_channel;
pub mod share;
pub mod shm_pipe;

pub use secure_channel::{SecureChannelManager, SecureIpcError, SecureSession, SignedIpcFrame, SECURE_IPC};
pub use share::{ShareSheetManager, ShareTarget, ShareTransaction, SHARE_SHEET_MANAGER, IPC_MSG_AMCP_SHARE};
pub use shm_pipe::{ShmError, ShmPipeManager, ShmRingBuffer, ShmSlot, DmaDescriptor, SHM_PIPE_MANAGER};

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;

use crate::capability::{CapError, CapabilityTable, CapabilityType, Rights};
use crate::task::TaskId;

/// Maximum payload size for high-speed microkernel IPC messages.
pub const IPC_MAX_PAYLOAD: usize = 64;

/// Capacity of each task's incoming message mailbox.
pub const MAILBOX_CAPACITY: usize = 16;

/// A discrete IPC message exchanged between two tasks.
#[derive(Debug, Clone)]
pub struct IpcMessage {
    /// Identity of the transmitting task.
    pub sender: TaskId,
    /// Protocol or application-defined message type code.
    pub msg_type: u32,
    /// Fixed-size zero-copy inline payload buffer.
    pub payload: [u8; IPC_MAX_PAYLOAD],
    /// Length of active data in the payload buffer.
    pub len: usize,
}

impl IpcMessage {
    /// Constructs a new IPC message from a byte slice.
    pub fn new(sender: TaskId, msg_type: u32, data: &[u8]) -> Self {
        let len = data.len().min(IPC_MAX_PAYLOAD);
        let mut payload = [0u8; IPC_MAX_PAYLOAD];
        payload[..len].copy_from_slice(&data[..len]);
        Self {
            sender,
            msg_type,
            payload,
            len,
        }
    }
}

/// Errors returned by the IPC subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// Capability validation failed (unauthorized communication attempt).
    CapabilityDenied(CapError),
    /// The target task does not exist or has terminated.
    TargetNotFound,
    /// The recipient's mailbox is full.
    MailboxFull,
    /// No messages available in the mailbox.
    MailboxEmpty,
}

impl From<CapError> for IpcError {
    fn from(err: CapError) -> Self {
        IpcError::CapabilityDenied(err)
    }
}

/// A task's incoming message queue.
struct TaskMailbox {
    task_id: TaskId,
    queue: VecDeque<IpcMessage>,
}

/// Global system IPC router managing mailboxes for all tasks.
pub struct IpcRouter {
    mailboxes: Vec<TaskMailbox>,
}

impl IpcRouter {
    pub const fn new() -> Self {
        Self {
            mailboxes: Vec::new(),
        }
    }

    /// Registers a mailbox for a newly spawned task.
    pub fn register_task(&mut self, task_id: TaskId) {
        if !self.mailboxes.iter().any(|m| m.task_id == task_id) {
            self.mailboxes.push(TaskMailbox {
                task_id,
                queue: VecDeque::with_capacity(MAILBOX_CAPACITY),
            });
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

        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.task_id == dest_task)
            .ok_or(IpcError::TargetNotFound)?;

        if mailbox.queue.len() >= MAILBOX_CAPACITY {
            return Err(IpcError::MailboxFull);
        }

        let msg = IpcMessage::new(sender, msg_type, data);
        mailbox.queue.push_back(msg);
        crate::event::post_event(dest_task, crate::event::EVENT_IPC_MSG);
        Ok(())
    }

    /// Delivers an authenticated system or kernel message directly to the target mailbox (e.g. AMCP Share Sheet bus).
    pub fn deliver_system_message(&mut self, dest: TaskId, msg: IpcMessage) -> Result<(), IpcError> {
        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.task_id == dest)
            .ok_or(IpcError::TargetNotFound)?;

        if mailbox.queue.len() >= MAILBOX_CAPACITY {
            return Err(IpcError::MailboxFull);
        }

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

        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.task_id == receiver)
            .ok_or(IpcError::TargetNotFound)?;

        Ok(mailbox.queue.pop_front())
    }

    /// Sends a pre-verified message directly to the target mailbox.
    pub fn send_direct(
        &mut self,
        sender: TaskId,
        dest_task: TaskId,
        msg_type: u32,
        data: &[u8],
    ) -> Result<(), IpcError> {
        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.task_id == dest_task)
            .ok_or(IpcError::TargetNotFound)?;

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
}

/// Global system-wide IPC message router.
pub static IPC_ROUTER: Mutex<IpcRouter> = Mutex::new(IpcRouter::new());

/// Initializes the IPC subsystem, secure channels, share bus, and zero-copy SHM pipes.
pub fn init() {
    secure_channel::init();
    share::init();
    shm_pipe::init();
}

