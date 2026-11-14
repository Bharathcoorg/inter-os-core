// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
        // Clear SUM. Firmware leaves supervisor user-memory access enabled, which
        // would let S-mode reach user pages regardless of the permissions encoded
        // in the page tables. It is re-enabled only around an explicit copy.
        "csrrc zero, sstatus, {2}",
        in(reg) trap_handler_addr,
        in(reg) stack_top,
        in(reg) crate::mmu::SSTATUS_SUM,
        options(nostack)
    );
}

/// Reads `sstatus` so the caller can verify the supervisor isolation posture.
pub fn read_sstatus() -> usize {
    let value: usize;
    unsafe {
        asm!("csrr {0}, sstatus", out(reg) value);
    }
    value
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

        // Restore the user SP directly and keep sscratch bound to the kernel stack.
        // The previous implementation left the user SP in sscratch, causing a
        // second user trap to allocate its TrapFrame on the user stack.
        "ld t0,  8*2(sp)",
        "addi sp, sp, 288",
        "csrw sscratch, sp",
        "mv sp, t0",
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
                // Step over the ecall itself. A compressed `c.ecall` is 16 bits,
                // so a fixed +4 would resume in the middle of the next instruction.
                let width = unsafe { crate::mmu::instruction_len(tf.sepc) };
                tf.sepc += width;
                crate::syscall::dispatch_syscall(tf);
                if syscall_num == 2 || syscall_num == 64 {
                    if unsafe { IN_LIFECYCLE_TEST } {
                        unsafe {
                            return_to_kernel_mode();
                        }
                    }
                    static mut INIT_LAUNCHED: bool = false;
                    if unsafe { !INIT_LAUNCHED } {
                        unsafe { INIT_LAUNCHED = true; }
                        println!("--- [Spawning Userspace PID 1 (init) in U-Mode] ---");
                        match crate::elf::load_elf(crate::images::USER_INIT_ELF) {
                            Ok(loaded) => {
                                println!("[BOOT] RISC-V PID 1 ELF loaded: entry 0x{:08x}, sp 0x{:08x}", loaded.entry_point, loaded.stack_top);
                                {
                                    let mut sched = crate::task::SCHEDULER.lock();
                                    if !sched.tasks.is_empty() {
                                        sched.tasks[0].name = "init";
                                        sched.tasks[0].state = crate::task::TaskState::Running;
                                        sched.tasks[0].page_table = Some(loaded.address_space.satp);
                                        sched.tasks[0].user_entry = loaded.entry_point;
                                        sched.tasks[0].user_stack_top = loaded.stack_top;
                                        let _ = sched.tasks[0].cap_table.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
                                        let _ = sched.tasks[0].cap_table.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
                                        let _ = sched.tasks[0].cap_table.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE, None);
                                        let _ = sched.tasks[0].cap_table.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);
                                        let _ = sched.tasks[0].cap_table.mint(crate::capability::CapabilityType::TimerTickRead, crate::capability::Rights::READ, None);
                                        loaded.address_space.try_activate().expect("Failed to activate init address space");
                                        sched.tasks[0].address_space = Some(loaded.address_space);
                                    }
                                }
                                tf.sepc = loaded.entry_point;
                                tf.regs[2] = loaded.stack_top; // sp
                                tf.regs[10] = 0; // a0
                                tf.regs[11] = 0; // a1
                                tf.regs[12] = 0; // a2
                                tf.regs[17] = 0; // a7
                                return;
                            }
                            Err(e) => {
                                println!("[BOOT] Failed to load RISC-V init ELF: {}", e);
                            }
                        }
                    }
                    let exited = crate::task::SCHEDULER.lock().exit_current();
                    println!("[INIT] Userspace exit recorded by scheduler: {}", exited);
                    crate::task::yield_now();
                    loop {
                        unsafe { core::arch::asm!("wfi"); }
                    }
                }
            }
            // Environment call from S-mode
            9 => {
                let width = unsafe { crate::mmu::instruction_len(tf.sepc) };
                tf.sepc += width;
                crate::syscall::dispatch_syscall(tf);
            }
            // Instruction Page Fault (12), Load Page Fault (13), Store Page Fault (15)
            12 | 13 | 15 => {
                println!(
                    "[TRAP:PAGE_FAULT] Fault at addr: 0x{:016x}, sepc: 0x{:016x}, cause: {}",
                    tf.stval, tf.sepc, code
                );
                tf.regs[10] = (-14isize) as usize; // -EFAULT
                let exited = crate::task::SCHEDULER.lock().exit_current();
                println!("[TRAP] Faulted userspace task marked exited: {}", exited);
                loop {
                    unsafe { core::arch::asm!("wfi"); }
                }
            }
            // Illegal instruction
            2 => {
                println!("[TRAP:ILLEGAL_INSN] sepc: 0x{:016x}, val: 0x{:016x}", tf.sepc, tf.stval);
                let exited = crate::task::SCHEDULER.lock().exit_current();
                println!("[TRAP] Illegal-instruction task marked exited: {}", exited);
                loop {
                    unsafe { core::arch::asm!("wfi"); }
                }
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
        // 2. Clear SPP (bit 8) so the return lands in U-mode, set SPIE (bit 5), and
        // clear SUM (bit 18) so the privilege drop carries no supervisor
        // user-memory bypass across the boundary.
        "csrr t0, sstatus",
        "li t1, ~((1 << 8) | (1 << 18))",
        "and t0, t0, t1",
        "ori t0, t0, (1 << 5)",
        // Clear VS (bits [10:9]) to strictly trap and isolate vector state
        "li t1, ~(3 << 9)",
        "and t0, t0, t1",
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct KernelSavedContext {
    pub ra: usize,
    pub sp: usize,
    pub s0: usize,
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub s8: usize,
    pub s9: usize,
    pub s10: usize,
    pub s11: usize,
}

pub static mut KERNEL_SAVED_CTX: KernelSavedContext = KernelSavedContext {
    ra: 0,
    sp: 0,
    s0: 0, s1: 0, s2: 0, s3: 0, s4: 0, s5: 0, s6: 0, s7: 0, s8: 0, s9: 0, s10: 0, s11: 0,
};

pub static mut IN_LIFECYCLE_TEST: bool = false;

/// Drops CPU execution privilege from S-mode to U-mode via `sret`, saving kernel context
/// so `return_to_kernel_mode` can return back to kernel execution.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user_mode_with_return(entry_point: usize, user_sp: usize, kstack_top: usize) {
    naked_asm!(
        // Save callee-saved registers to KERNEL_SAVED_CTX
        "la t0, {ctx}",
        "sd ra, 0(t0)",
        "sd sp, 8(t0)",
        "sd s0, 16(t0)",
        "sd s1, 24(t0)",
        "sd s2, 32(t0)",
        "sd s3, 40(t0)",
        "sd s4, 48(t0)",
        "sd s5, 56(t0)",
        "sd s6, 64(t0)",
        "sd s7, 72(t0)",
        "sd s8, 80(t0)",
        "sd s9, 88(t0)",
        "sd s10, 96(t0)",
        "sd s11, 104(t0)",

        // 1. Initialize sscratch with kernel stack for handling traps from U-mode
        "csrw sscratch, a2",
        // 2. Clear SPP (bit 8) so return lands in U-mode, set SPIE (bit 5), clear SUM (bit 18)
        "csrr t0, sstatus",
        "li t1, ~((1 << 8) | (1 << 18))",
        "and t0, t0, t1",
        "ori t0, t0, (1 << 5)",
        // Clear VS (bits [10:9]) to strictly trap and isolate vector state
        "li t1, ~(3 << 9)",
        "and t0, t0, t1",
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
        ctx = sym KERNEL_SAVED_CTX,
    );
}

/// Restores kernel callee-saved registers and returns to caller of `enter_user_mode_with_return`.
#[unsafe(naked)]
pub unsafe extern "C" fn return_to_kernel_mode() -> ! {
    naked_asm!(
        "la t0, {ctx}",
        "ld ra, 0(t0)",
        "ld sp, 8(t0)",
        "ld s0, 16(t0)",
        "ld s1, 24(t0)",
        "ld s2, 32(t0)",
        "ld s3, 40(t0)",
        "ld s4, 48(t0)",
        "ld s5, 56(t0)",
        "ld s6, 64(t0)",
        "ld s7, 72(t0)",
        "ld s8, 80(t0)",
        "ld s9, 88(t0)",
        "ld s10, 96(t0)",
        "ld s11, 104(t0)",
        "ret",
        ctx = sym KERNEL_SAVED_CTX,
    );
}

