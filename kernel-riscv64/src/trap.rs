//! RISC-V 64 Trap & Exception Management
//!
//! Handles synchronous exceptions (ecall, page faults, illegal instructions)
//! and asynchronous hardware interrupts with per-task kernel stacks.

use core::arch::{asm, naked_asm};
use crate::println;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TrapFrame {
    pub regs: [usize; 32], // x0..x31: ra(x1), sp(x2), gp(x3), tp(x4), t0-t2(x5-x7), s0-s1(x8-x9), a0-a7(x10-x17), s2-s11(x18-x27), t3-t6(x28-x31)
    pub sstatus: usize,
    pub sepc: usize,
    pub scause: usize,
    pub stval: usize,
}

// Global kernel stack for trap handling if task stack is unavailable
static mut TRAP_KERNEL_STACK: [u8; 16384] = [0; 16384];

/// Initializes RISC-V trap vectors by setting `stvec` and `sscratch`.
pub unsafe fn init_traps() {
    let trap_handler_addr = trap_entry as *const () as usize;
    let stack_top = (&raw const TRAP_KERNEL_STACK) as usize + 16384;
    // Direct mode: set bottom 2 bits to 0
    asm!(
        "csrw stvec, {0}",
        "csrw sscratch, {1}",
        "csrrs zero, sstatus, {2}", // Enable SUM (bit 18) for servicing U-mode syscall memory
        in(reg) trap_handler_addr,
        in(reg) stack_top,
        in(reg) (1usize << 18),
        options(nostack)
    );
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn trap_entry() -> ! {
    naked_asm!(
        // 1. Swap user SP with kernel stack in sscratch
        "csrrw sp, sscratch, sp",
        // 2. Allocate space for TrapFrame (32 regs * 8 + 4 CSRs * 8 = 288 bytes)
        "addi sp, sp, -288",
        // 3. Save general registers
        "sd x1,  8*1(sp)",
        // Save original SP (currently in sscratch)
        "csrr t0, sscratch",
        "sd t0,  8*2(sp)",
        "sd x3,  8*3(sp)",
        "sd x4,  8*4(sp)",
        "sd x5,  8*5(sp)",
        "sd x6,  8*6(sp)",
        "sd x7,  8*7(sp)",
        "sd x8,  8*8(sp)",
        "sd x9,  8*9(sp)",
        "sd x10, 8*10(sp)",
        "sd x11, 8*11(sp)",
        "sd x12, 8*12(sp)",
        "sd x13, 8*13(sp)",
        "sd x14, 8*14(sp)",
        "sd x15, 8*15(sp)",
        "sd x16, 8*16(sp)",
        "sd x17, 8*17(sp)",
        "sd x18, 8*18(sp)",
        "sd x19, 8*19(sp)",
        "sd x20, 8*20(sp)",
        "sd x21, 8*21(sp)",
        "sd x22, 8*22(sp)",
        "sd x23, 8*23(sp)",
        "sd x24, 8*24(sp)",
        "sd x25, 8*25(sp)",
        "sd x26, 8*26(sp)",
        "sd x27, 8*27(sp)",
        "sd x28, 8*28(sp)",
        "sd x29, 8*29(sp)",
        "sd x30, 8*30(sp)",
        "sd x31, 8*31(sp)",

        // 4. Save CSRs
        "csrr t1, sstatus",
        "csrr t2, sepc",
        "csrr t3, scause",
        "csrr t4, stval",
        "sd t1, 8*32(sp)",
        "sd t2, 8*33(sp)",
        "sd t3, 8*34(sp)",
        "sd t4, 8*35(sp)",

        // 5. Call Rust trap handler: a0 = &mut TrapFrame
        "mv a0, sp",
        "call trap_handler",

        // 6. Restore CSRs (sepc and sstatus may have been updated)
        "ld t1, 8*32(sp)",
        "ld t2, 8*33(sp)",
        "csrw sstatus, t1",
        "csrw sepc, t2",

        // 7. Restore general registers
        "ld x1,  8*1(sp)",
        "ld x3,  8*3(sp)",
        "ld x4,  8*4(sp)",
        "ld x5,  8*5(sp)",
        "ld x6,  8*6(sp)",
        "ld x7,  8*7(sp)",
        "ld x8,  8*8(sp)",
        "ld x9,  8*9(sp)",
        "ld x10, 8*10(sp)",
        "ld x11, 8*11(sp)",
        "ld x12, 8*12(sp)",
        "ld x13, 8*13(sp)",
        "ld x14, 8*14(sp)",
        "ld x15, 8*15(sp)",
        "ld x16, 8*16(sp)",
        "ld x17, 8*17(sp)",
        "ld x18, 8*18(sp)",
        "ld x19, 8*19(sp)",
        "ld x20, 8*20(sp)",
        "ld x21, 8*21(sp)",
        "ld x22, 8*22(sp)",
        "ld x23, 8*23(sp)",
        "ld x24, 8*24(sp)",
        "ld x25, 8*25(sp)",
        "ld x26, 8*26(sp)",
        "ld x27, 8*27(sp)",
        "ld x28, 8*28(sp)",
        "ld x29, 8*29(sp)",
        "ld x30, 8*30(sp)",
        "ld x31, 8*31(sp)",

        // Restore original SP into sscratch, deallocate frame, and restore SP
        "ld t0,  8*2(sp)",
        "csrw sscratch, t0",
        "addi sp, sp, 288",
        "csrrw sp, sscratch, sp",
        "sret",
    );
}

#[no_mangle]
pub extern "C" fn trap_handler(tf: &mut TrapFrame) {
    let scause = tf.scause;
    let is_interrupt = (scause >> 63) != 0;
    let code = scause & 0x7FFF_FFFF_FFFF_FFFF;

    if is_interrupt {
        // Supervisor Software Interrupt (1), Supervisor Timer (5), External (9)
        match code {
            5 => {
                // Clear timer interrupt via SBI / reload timer
            }
            _ => {
                println!("[TRAP:IRQ] Unhandled interrupt code: {}", code);
            }
        }
    } else {
        match code {
            // Environment call from U-mode (User Syscall)
            8 => {
                let syscall_num = tf.regs[17];
                tf.sepc += 4; // Advance instruction pointer past ecall
                crate::syscall::dispatch_syscall(tf);
                if syscall_num == 2 {
                    println!("[INIT] Userspace init process completed execution. Microkernel entering low-power state.");
                    loop {
                        unsafe {
                            core::arch::asm!("wfi");
                        }
                    }
                }
            }
            // Environment call from S-mode
            9 => {
                tf.sepc += 4;
                crate::syscall::dispatch_syscall(tf);
            }
            // Instruction Page Fault (12), Load Page Fault (13), Store Page Fault (15)
            12 | 13 | 15 => {
                println!(
                    "[TRAP:PAGE_FAULT] Fault at addr: 0x{:016x}, sepc: 0x{:016x}, cause: {}",
                    tf.stval, tf.sepc, code
                );
                // Gracefully kill user process or halt
                tf.regs[10] = (-14isize) as usize; // -EFAULT
            }
            // Illegal instruction
            2 => {
                println!("[TRAP:ILLEGAL_INSN] sepc: 0x{:016x}, val: 0x{:016x}", tf.sepc, tf.stval);
                tf.sepc += 4;
            }
            _ => {
                println!(
                    "[TRAP:EXCEPTION] scause: {}, sepc: 0x{:016x}, stval: 0x{:016x}",
                    code, tf.sepc, tf.stval
                );
            }
        }
    }
}

/// Returns the top address of the primordial kernel trap stack.
pub fn kernel_stack_top() -> usize {
    (&raw const TRAP_KERNEL_STACK) as usize + 16384
}

/// Drops CPU execution privilege from S-mode to U-mode via `sret`.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user_mode(entry_point: usize, user_sp: usize, kstack_top: usize) -> ! {
    naked_asm!(
        // a0 = entry_point, a1 = user_sp, a2 = kstack_top
        // 1. Initialize sscratch with kernel stack for handling traps from U-mode
        "csrw sscratch, a2",
        // 2. Clear SPP (bit 8 = 0 for U-mode return) and set SPIE (bit 5 = 1)
        "csrr t0, sstatus",
        "li t1, ~(1 << 8)",
        "and t0, t0, t1",
        "ori t0, t0, (1 << 5)",
        "csrw sstatus, t0",
        // 3. Set sepc = entry_point (a0)
        "csrw sepc, a0",
        // 4. Set user SP = a1
        "mv sp, a1",
        // 5. Zero registers for process boundary isolation
        "mv a0, zero",
        "mv a1, zero",
        "mv a2, zero",
        "mv a3, zero",
        "mv a4, zero",
        "mv a5, zero",
        "mv a6, zero",
        "mv a7, zero",
        // 6. Hardware privilege drop into U-mode at sepc
        "sret",
    );
}

