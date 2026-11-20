// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign IPC Subsystem and High-Assurance Memory-Message Facilities
//!
//! Exposes register-based inline scalar IPC, lifetime-bound memory lending,
//! single-ownership transfers, and high-rate circular streaming channels.

pub mod memory;

pub use memory::{
    BorrowedMutableBuffer, BorrowedReadOnlyBuffer, IpcMemoryError, ScalarMessage, StreamSlot,
    StreamingSharedMemory, TransferredBuffer,
};

/// A copy of a dispatched input event the kernel also posts to the focused task.
pub const MSG_INPUT_EVENT: u32 = 0x494E_5054;

/// The notification that a window took focus, likewise posted into that task's mailbox.
pub const MSG_WINDOW_FOCUSED: u32 = 0x7001;

/// Sends a raw IPC message with payload bytes to the target task.
pub fn send(target_task_id: u64, msg_type: u32, payload: &[u8]) -> Result<(), i64> {
    crate::ipc_send(target_task_id, msg_type, payload)
}

/// Receives an IPC message from any sender, returning `(sender_task_id, msg_type, bytes_read)`.
pub fn receive_any(out_buf: &mut [u8]) -> Result<(u64, u32, usize), i64> {
    let mut sender = 0u64;
    let mut msg_type = 0u32;
    let ret = unsafe {
        crate::syscall5(
            crate::SyscallNumber::IpcRecvMeta,
            0,
            out_buf.as_mut_ptr() as u64,
            out_buf.len() as u64,
            &mut sender as *mut u64 as u64,
            &mut msg_type as *mut u32 as u64,
        )
    };
    if ret >= 0 {
        Ok((sender, msg_type, ret as usize))
    } else {
        Err(ret)
    }
}
