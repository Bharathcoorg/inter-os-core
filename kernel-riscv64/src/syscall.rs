// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 System Call Dispatcher & Pointer Validation
//!
//! Provides zero-trust system call execution in S-mode on behalf of U-mode applications.
//! Implements strict pointer validation ensuring user pointers cannot access kernel
//! memory or MMIO ranges.

use crate::trap::TrapFrame;
use crate::println;
use crate::task::SCHEDULER;

fn validate_user_range(addr: usize, len: usize, write: bool) -> Result<(), i64> {
    if len == 0 || addr < USER_SPACE_MIN { return if len == 0 { Ok(()) } else { Err(-14) }; }
    let end = addr.checked_add(len).ok_or(-14)?;
    if end > USER_SPACE_MAX { return Err(-14); }

    let first = addr & !(crate::mmu::PAGE_SIZE - 1);
    let last = (end - 1) & !(crate::mmu::PAGE_SIZE - 1);
    let mut page = first;
    loop {
        if page == crate::mmu::USER_CODE_VADDR {
            if write { return Err(-13); }
        } else if page != crate::mmu::USER_CODE_VADDR + crate::mmu::PAGE_SIZE {
            return Err(-14);
        }
        if page == last { break; }
        page = page.checked_add(crate::mmu::PAGE_SIZE).ok_or(-14)?;
    }
    Ok(())
}

fn copy_from_user(dst: &mut [u8], addr: usize, len: usize) -> Result<(), i64> {
    if len > dst.len() { return Err(-22); }
    validate_user_range(addr, len, false)?;
    // The range is validated first, so SUM is only ever enabled for an address
    // that the page tables mark as user-readable.
    unsafe {
        crate::mmu::with_user_memory(|| {
            core::ptr::copy_nonoverlapping(addr as *const u8, dst.as_mut_ptr(), len);
        });
    }
    Ok(())
}

#[allow(dead_code)]
fn copy_to_user(addr: usize, src: &[u8]) -> Result<(), i64> {
    validate_user_range(addr, src.len(), true)?;
    unsafe {
        crate::mmu::with_user_memory(|| {
            core::ptr::copy_nonoverlapping(src.as_ptr(), addr as *mut u8, src.len());
        });
    }
    Ok(())
}

pub const SYS_YIELD: usize = 1;
pub const SYS_EXIT: usize = 2;
pub const SYS_LOG: usize = 3;
pub const SYS_GET_TICKS: usize = 4;
pub const SYS_IPC_SEND: usize = 5;
pub const SYS_IPC_RECV: usize = 6;
pub const SYS_GET_TASK_ID: usize = 7;
pub const SYS_FS_OPEN: usize = 8;
pub const SYS_FS_READ: usize = 9;
pub const SYS_FS_WRITE: usize = 10;
pub const SYS_FS_CLOSE: usize = 11;
pub const SYS_EVENT_POLL: usize = 62;
pub const SYS_EXIT_WITH_CODE: usize = 64;
pub const SYS_TOUCH_POLL: usize = 71;

pub const USER_SPACE_MIN: usize = 0x0000_1000;
pub const USER_SPACE_MAX: usize = 0x7FFF_FFFF; // Below 2GB Kernel DRAM
pub const KERNEL_DRAM_START: usize = 0x8000_0000;

/// Dispatches an incoming system call from the given trap frame.
pub fn dispatch_syscall(tf: &mut TrapFrame) {
    let num = tf.regs[17]; // a7
    let a0 = tf.regs[10];  // a0
    let a1 = tf.regs[11];  // a1
    let a2 = tf.regs[12];  // a2
    let _a3 = tf.regs[13]; // a3

    let res: i64 = match num {
        // 1: Yield
        1 => {
            0
        }
        // 2: Exit
        2 => {
            println!("[SYSCALL:EXIT] Task exited with code {}", a0);
            0
        }
        // 3: Log (cap_slot, msg_ptr, msg_len)
        3 => {
            let msg_ptr = a1;
            let msg_len = a2;

            if validate_user_range(msg_ptr, msg_len, false).is_err() {
                -14 // -EFAULT
            } else if msg_len > 1024 {
                -22 // -EINVAL
            } else {
                let mut kbuf = [0u8; 1024];
                if copy_from_user(&mut kbuf, msg_ptr, msg_len).is_err() {
                    -14
                } else if let Ok(s) = core::str::from_utf8(&kbuf[..msg_len]) {
                    println!("[USER:LOG] {}", s);
                    0
                } else {
                    -22 // -EINVAL (Invalid UTF-8)
                }
            }
        }
        // 4: GetTicks
        4 => {
            let ticks: u64;
            unsafe {
                core::arch::asm!("rdtime {}", out(reg) ticks);
            }
            ticks as i64
        }
        // 5: IpcSend (cap_slot, target_id, msg_type, payload_ptr, payload_len)
        // IPC is not yet backed by a bounded RISC-V mailbox implementation. Do
        // not report success until the capability and queue semantics exist.
        5 => -38,
        // 6: IpcRecv (cap_slot, out_buf_ptr, out_buf_len)
        6 => -38,
        // 7: GetTaskId
        7 => {
            SCHEDULER.lock().current_task_id().0 as i64
        }
        // 8: FsOpen (cap_slot, path_ptr, path_len, flags)
        8 => {
            let path_ptr = a1;
            let path_len = a2;
            if path_len > 0 && validate_user_range(path_ptr, path_len, false).is_err() {
                -14 // -EFAULT
            } else {
                -13 // -EACCES: fail-closed capability check
            }
        }
        // 9: FsRead (cap_slot, fd, buf_ptr, buf_len)
        9 => {
            let buf_ptr = a2;
            let buf_len = _a3;
            if buf_len > 0 && validate_user_range(buf_ptr, buf_len, true).is_err() {
                -14 // -EFAULT
            } else {
                -13 // -EACCES: fail-closed capability check
            }
        }
        // 10: FsWrite (cap_slot, fd, buf_ptr, buf_len)
        10 => {
            let buf_ptr = a2;
            let buf_len = _a3;
            if buf_len > 0 && validate_user_range(buf_ptr, buf_len, false).is_err() {
                -14 // -EFAULT
            } else {
                -13 // -EACCES: fail-closed capability check
            }
        }
        // 11: FsClose (cap_slot, fd)
        11 => {
            -13 // -EACCES: fail-closed capability check
        }
        // 62: EventPoll (cap_slot, interest_mask, timeout_ticks)
        62 => {
            -13 // -EACCES: fail-closed capability check
        }
        // 64: ExitWithCode (exit_code)
        64 => {
            println!("[SYSCALL:EXIT] Task exited with code {}", a0);
            SCHEDULER.lock().exit_current();
            0
        }
        // 71: TouchPoll (cap_slot)
        71 => {
            -13 // -EACCES: fail-closed capability check
        }
        _ => {
            -38 // -ENOSYS
        }
    };

    tf.regs[10] = res as usize;
}
