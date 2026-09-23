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
            let ptr = a0 as *const u8;
            let len = (a1 as usize).min(256);
            if ptr.is_null() || len == 0 {
                return -1;
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

            unsafe {
                let slice = core::slice::from_raw_parts(ptr, len);
                if let Ok(s) = core::str::from_utf8(slice) {
                    crate::kprintln!("[LOG] {}", s);
                    return 0;
                }
            }
            -22 // -EINVAL
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
            let ptr = a2 as *const u8;
            let len = (a3 as usize).min(IPC_MAX_PAYLOAD);

            let sched = SCHEDULER.lock();
            let curr_idx = sched.current_task_idx;
            if curr_idx >= sched.tasks.len() {
                return -1;
            }
            let sender = sched.tasks[curr_idx].id;
            let caps = sched.tasks[curr_idx].cap_table.clone();
            drop(sched);

            let data = if ptr.is_null() || len == 0 {
                &[]
            } else {
                unsafe { core::slice::from_raw_parts(ptr, len) }
            };

            let mut router = IPC_ROUTER.lock();
            match router.send(&caps, sender, recipient, msg_type, data) {
                Ok(()) => 0,
                Err(_) => -13, // -EACCES
            }
        }
        SYS_IPC_RECV => {
            let out_buf = a0 as *mut u8;
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
                    if !out_buf.is_null() && max_len > 0 {
                        let copy_len = msg.len.min(max_len);
                        unsafe {
                            core::ptr::copy_nonoverlapping(msg.payload.as_ptr(), out_buf, copy_len);
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
