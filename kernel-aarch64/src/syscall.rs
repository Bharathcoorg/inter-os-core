// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Unified System Call Interface (Syscall ABI)
//!
//! Conforms to standard AArch64 `svc #0` conventions:
//! - Syscall Number: `x8`
//! - Arguments: `x0` - `x4`
//! - Return Value: `x0`
//!
//! All system calls are strictly capability-gated against the calling task's CapabilityTable.

use crate::capability::{CapabilityType, Rights};
use crate::ipc::{IPC_ROUTER, IPC_MAX_PAYLOAD};
use crate::task::{SCHEDULER, yield_now};

pub const SYS_YIELD: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_LOG: u64 = 3;
pub const SYS_GET_TICKS: u64 = 4;
pub const SYS_IPC_SEND: u64 = 5;
pub const SYS_IPC_RECV: u64 = 6;
pub const SYS_GET_TASK_ID: u64 = 7;

fn validate_user_range(addr: u64, len: usize, write: bool) -> Result<(), i64> {
    const START: u64 = 0x0000_1000;
    const END: u64 = 0x0000_8000_0000_0000;
    const CODE_START: u64 = 0x0040_0000;
    const CODE_END: u64 = CODE_START + 0x0020_0000;
    const STACK_START: u64 = 0x0080_0000;
    const STACK_END: u64 = STACK_START + 0x0020_0000;
    if addr < START || len == 0 { return if len == 0 { Ok(()) } else { Err(-14) }; }
    let end = addr.checked_add(len as u64).ok_or(-14)?;
    if end > END { return Err(-14); }
    let in_code = addr >= CODE_START && end <= CODE_END;
    let in_stack = addr >= STACK_START && end <= STACK_END;
    if !(in_code || in_stack) { return Err(-14); }
    if write && in_code { return Err(-13); }
    Ok(())
}

fn copy_from_user(dst: &mut [u8], addr: u64, len: usize) -> Result<(), i64> {
    if len > dst.len() { return Err(-22); }
    validate_user_range(addr, len, false)?;
    unsafe { core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len); }
    Ok(())
}

fn copy_to_user(addr: u64, src: &[u8]) -> Result<(), i64> {
    validate_user_range(addr, src.len(), true)?;
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len()); }
    Ok(())
}


pub fn sys_dispatch(syscall_nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, _a4: u64) -> i64 {
    match syscall_nr {
        SYS_YIELD => {
            yield_now();
            0
        }
        SYS_EXIT => {
            let code = a0 as i32;
            crate::kprintln!("[SYSCALL] Task exited with code: {}", code);
            crate::user::record_el0_exit(code);
            0
        }
        SYS_LOG => {
            let addr = a0 as u64;
            let len = (a1 as usize).min(256);
            if validate_user_range(addr, len, false).is_err() {
                return -14; // -EFAULT
            }

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::SerialOutput, Rights::WRITE).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);

            let mut kbuf = [0u8; 256];
            if copy_from_user(&mut kbuf, addr, len).is_err() {
                return -14;
            }
            if let Ok(s) = core::str::from_utf8(&kbuf[..len]) {
                crate::kprintln!("[LOG] {}", s);
                0
            } else {
                -22 // -EINVAL
            }
        }
        SYS_GET_TICKS => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                let task = &sched.tasks[curr_idx];
                if task.cap_table.check(CapabilityType::TimerTickRead, Rights::READ).is_err() {
                    return -13; // -EACCES
                }
            }
            drop(sched);

            let timer = crate::timer::ArmGenericTimer::new();
            timer.read_counter() as i64
        }
        SYS_IPC_SEND => {
            let recipient = a0 as u32;
            let msg_type = a1 as u32;
            let addr = a2 as u64;
            let len = (a3 as usize).min(IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx >= sched.tasks.len() {
                return -1;
            }
            let sender = sched.tasks[curr_idx].id;
            let caps = sched.tasks[curr_idx].cap_table.clone();
            drop(sched);

            let mut kbuf = [0u8; IPC_MAX_PAYLOAD];
            if len > 0 {
                if copy_from_user(&mut kbuf, addr, len).is_err() {
                    return -14; // -EFAULT
                }
            }

            let mut router = IPC_ROUTER.lock();
            match router.send(&caps, sender, recipient, msg_type, &kbuf[..len]) {
                Ok(()) => 0,
                Err(_) => -13, // -EACCES
            }
        }
        SYS_IPC_RECV => {
            let out_addr = a0 as u64;
            let max_len = a1 as usize;

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx >= sched.tasks.len() {
                return -1;
            }
            let receiver = sched.tasks[curr_idx].id;
            let caps = sched.tasks[curr_idx].cap_table.clone();
            drop(sched);

            let mut router = IPC_ROUTER.lock();
            match router.receive(&caps, receiver) {
                Ok(msg) => {
                    if max_len > 0 {
                        let copy_len = msg.len.min(max_len);
                        if copy_to_user(out_addr, &msg.payload[..copy_len]).is_err() {
                            return -14; // -EFAULT
                        }
                    }
                    msg.len as i64
                }
                Err(_) => -11, // -EAGAIN (No message)
            }
        }
        SYS_GET_TASK_ID => {
            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx < sched.tasks.len() {
                sched.tasks[curr_idx].id as i64
            } else {
                0
            }
        }
        _ => -38, // -ENOSYS
    }
}
