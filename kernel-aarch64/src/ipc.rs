// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Capability-Guarded Inter-Process Communication (IPC)
//!
//! Provides zero-copy mailbox queues per task.
//! Transmission is strictly enforced: tasks must possess `CapabilityType::IpcSend`
//! targeted at the recipient's `TaskId`. Reception requires `CapabilityType::IpcReceive`.

use spin::Mutex;

use crate::capability::{CapError, CapabilityTable, CapabilityType, Rights};

pub const IPC_MAX_PAYLOAD: usize = 64;
pub const MAILBOX_CAPACITY: usize = 8;
pub const MAX_TASKS: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct IpcMessage {
    pub sender: u32,
    pub msg_type: u32,
    pub payload: [u8; IPC_MAX_PAYLOAD],
    pub len: usize,
}

impl IpcMessage {
    pub fn new(sender: u32, msg_type: u32, data: &[u8]) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    CapabilityDenied(CapError),
    TargetNotFound,
    MailboxFull,
    MailboxEmpty,
}

#[derive(Clone, Copy)]
pub struct TaskMailbox {
    pub task_id: u32,
    pub active: bool,
    pub messages: [Option<IpcMessage>; MAILBOX_CAPACITY],
    pub head: usize,
    pub tail: usize,
    pub count: usize,
}

impl TaskMailbox {
    pub const fn empty() -> Self {
        Self {
            task_id: 0,
            active: false,
            messages: [None; MAILBOX_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    pub fn push(&mut self, msg: IpcMessage) -> Result<(), IpcError> {
        if self.count >= MAILBOX_CAPACITY {
            return Err(IpcError::MailboxFull);
        }
        self.messages[self.tail] = Some(msg);
        self.tail = (self.tail + 1) % MAILBOX_CAPACITY;
        self.count += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<IpcMessage, IpcError> {
        if self.count == 0 {
            return Err(IpcError::MailboxEmpty);
        }
        let msg = self.messages[self.head].take().ok_or(IpcError::MailboxEmpty)?;
        self.head = (self.head + 1) % MAILBOX_CAPACITY;
        self.count -= 1;
        Ok(msg)
    }
}

pub struct IpcRouter {
    mailboxes: [TaskMailbox; MAX_TASKS],
}

impl IpcRouter {
    pub const fn new() -> Self {
        Self {
            mailboxes: [TaskMailbox::empty(); MAX_TASKS],
        }
    }

    pub fn register_task(&mut self, task_id: u32) {
        for mb in self.mailboxes.iter_mut() {
            if mb.active && mb.task_id == task_id {
                return;
            }
        }
        for mb in self.mailboxes.iter_mut() {
            if !mb.active {
                *mb = TaskMailbox::empty();
                mb.active = true;
                mb.task_id = task_id;
                return;
            }
        }
    }

    pub fn unregister_task(&mut self, task_id: u32) {
        for mb in self.mailboxes.iter_mut() {
            if mb.active && mb.task_id == task_id {
                *mb = TaskMailbox::empty();
                return;
            }
        }
    }

    pub fn send(
        &mut self,
        sender_caps: &CapabilityTable,
        sender: u32,
        recipient: u32,
        msg_type: u32,
        data: &[u8],
    ) -> Result<(), IpcError> {
        sender_caps
            .check_targeted(CapabilityType::IpcSend, Rights::INVOKE, recipient)
            .map_err(IpcError::CapabilityDenied)?;

        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.active && m.task_id == recipient)
            .ok_or(IpcError::TargetNotFound)?;

        mailbox.push(IpcMessage::new(sender, msg_type, data))
    }

    pub fn receive(&mut self, receiver_caps: &CapabilityTable, receiver: u32) -> Result<IpcMessage, IpcError> {
        receiver_caps
            .check(CapabilityType::IpcReceive, Rights::READ)
            .map_err(IpcError::CapabilityDenied)?;

        let mailbox = self
            .mailboxes
            .iter_mut()
            .find(|m| m.active && m.task_id == receiver)
            .ok_or(IpcError::TargetNotFound)?;

        mailbox.pop()
    }
}

pub static IPC_ROUTER: Mutex<IpcRouter> = Mutex::new(IpcRouter::new());

