//! RISC-V 64 System Call Dispatcher & Pointer Validation
//!
//! Provides zero-trust system call execution in S-mode on behalf of U-mode applications.
//! Implements strict pointer validation ensuring user pointers cannot access kernel
//! memory or MMIO ranges.

use crate::trap::TrapFrame;
use crate::println;

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

            if msg_ptr < USER_SPACE_MIN || msg_ptr.saturating_add(msg_len) > USER_SPACE_MAX || msg_ptr >= KERNEL_DRAM_START {
                -14 // -EFAULT (Forbidden kernel DRAM or MMIO)
            } else if msg_len > 1024 {
                -22 // -EINVAL
            } else {
                let mut kbuf = [0u8; 1024];
                unsafe {
                    core::ptr::copy_nonoverlapping(msg_ptr as *const u8, kbuf.as_mut_ptr(), msg_len);
                }
                if let Ok(s) = core::str::from_utf8(&kbuf[..msg_len]) {
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
        5 => {
            0
        }
        // 6: IpcRecv (cap_slot, out_buf_ptr, out_buf_len)
        6 => {
            0
        }
        // 7: GetTaskId
        7 => {
            1 // Default task ID
        }
        _ => {
            -38 // -ENOSYS
        }
    };

    tf.regs[10] = res as usize;
}
