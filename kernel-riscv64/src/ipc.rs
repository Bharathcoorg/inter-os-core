// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Capability-Guarded Inter-Process Communication (IPC)
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

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

pub struct IpcRouter {
    pub mailboxes: [TaskMailbox; MAX_TASKS],
    pub total_sent: u64,
    pub total_delivered: u64,
}

impl IpcRouter {
    pub const fn new() -> Self {
        Self {
            mailboxes: [const { TaskMailbox::empty() }; MAX_TASKS],
            total_sent: 0,
            total_delivered: 0,
        }
    }

    pub fn register_task(&mut self, task_id: u32) {
        for mb in self.mailboxes.iter_mut() {
            if !mb.active {
                mb.task_id = task_id;
                mb.active = true;
                mb.head = 0;
                mb.tail = 0;
                mb.count = 0;
                return;
            }
        }
    }

    pub fn send(
        &mut self,
        sender_caps: &CapabilityTable,
        sender_id: u32,
        recipient_id: u32,
        msg_type: u32,
        data: &[u8],
    ) -> Result<(), IpcError> {
        sender_caps
            .check_targeted(CapabilityType::IpcSend, Rights::INVOKE, recipient_id)
            .map_err(IpcError::CapabilityDenied)?;

        let target_mb = self
            .mailboxes
            .iter_mut()
            .find(|mb| mb.active && mb.task_id == recipient_id)
            .ok_or(IpcError::TargetNotFound)?;

        let msg = IpcMessage::new(sender_id, msg_type, data);
        target_mb.push(msg)?;
        self.total_sent += 1;
        Ok(())
    }

    pub fn receive(
        &mut self,
        receiver_caps: &CapabilityTable,
        receiver_id: u32,
    ) -> Result<IpcMessage, IpcError> {
        receiver_caps
            .check(CapabilityType::IpcReceive, Rights::READ)
            .map_err(IpcError::CapabilityDenied)?;

        let target_mb = self
            .mailboxes
            .iter_mut()
            .find(|mb| mb.active && mb.task_id == receiver_id)
            .ok_or(IpcError::TargetNotFound)?;

        let msg = target_mb.pop()?;
        self.total_delivered += 1;
        Ok(msg)
    }

    pub fn receive_meta(
        &mut self,
        receiver_caps: &CapabilityTable,
        receiver_id: u32,
    ) -> Result<(u32, u32, usize), IpcError> {
        receiver_caps
            .check(CapabilityType::IpcReceive, Rights::READ)
            .map_err(IpcError::CapabilityDenied)?;

        let target_mb = self
            .mailboxes
            .iter()
            .find(|mb| mb.active && mb.task_id == receiver_id)
            .ok_or(IpcError::TargetNotFound)?;

        if target_mb.count == 0 {
            return Err(IpcError::MailboxEmpty);
        }

        let msg = target_mb.messages[target_mb.head].as_ref().ok_or(IpcError::MailboxEmpty)?;
        Ok((msg.sender, msg.msg_type, msg.len))
    }
}

pub static IPC_ROUTER: Mutex<IpcRouter> = Mutex::new(IpcRouter::new());
