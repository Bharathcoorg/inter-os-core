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

// Global kernel stack for trap handling if task stack is unavailable.
// Must be word-aligned: fat pointers formed on this stack (e.g. the `&[fmt::rt::Argument]`
// slice a `println!` builds in a trap handler) must never be odd, or `core::fmt`'s
// tagged-pointer encoding reads the pointer as a literal length and floods the console.
#[repr(C, align(16))]
struct TrapKernelStack([u8; 16384]);
static mut TRAP_KERNEL_STACK: TrapKernelStack = TrapKernelStack([0; 16384]);

/// Initializes RISC-V trap vectors by setting `stvec` and clearing `sscratch`.
/// `sscratch == 0` indicates S-mode execution; non-zero holds the active U-mode task kernel stack.
pub unsafe fn init_traps() {
    let trap_handler_addr = trap_entry as *const () as usize;
    let initial_sscratch: usize = 0;
    asm!(
        "csrw stvec, {0}",
        "csrw sscratch, {1}",
        // Clear SUM. Firmware leaves supervisor user-memory access enabled, which
        // would let S-mode reach user pages regardless of the permissions encoded
        // in the page tables. It is re-enabled only around an explicit copy.
        "csrrc zero, sstatus, {2}",
        in(reg) trap_handler_addr,
        in(reg) initial_sscratch,
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
        // 1. Distinguish U-mode (sscratch == kstack_top != 0) from S-mode (sscratch == 0)
        "csrrw sp, sscratch, sp",
        "bnez sp, 1f",
        // Trap originated in S-mode: sscratch holds interrupted kernel sp, sp is 0.
        "csrrw sp, sscratch, zero",
        "addi sp, sp, -288",
        "sd x1,  8*1(sp)",
        "sd t0,  8*5(sp)",
        "addi t0, sp, 288",
        "sd t0,  8*2(sp)",
        "j 2f",
        // Trap originated in U-mode: sp holds kstack_top, sscratch holds user sp.
        // Zero sscratch immediately so any nested S-mode trap detects S-mode.
        "1:",
        "addi sp, sp, -288",
        "sd x1,  8*1(sp)",
        "sd t0,  8*5(sp)",
        "csrrw t0, sscratch, zero",
        "sd t0,  8*2(sp)",
        "2:",
        "sd x3,  8*3(sp)",
        "sd x4,  8*4(sp)",
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

        // Check whether returning to U-mode (SPP==0 -> arm sscratch=kstack_top) or S-mode (SPP!=0 -> sscratch=0)
        "andi t0, t1, (1 << 8)",
        "bnez t0, 3f",
        "addi t0, sp, 288",
        "csrw sscratch, t0",
        "j 4f",
        "3:",
        "csrw sscratch, zero",
        "4:",

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

        // Restore original SP (user sp or interrupted kernel sp) last
        "ld sp,  8*2(sp)",
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
            // Environment call from S-mode is never a valid syscall path
            9 => {
                panic!("Environment call from S-mode at sepc=0x{:016x}", tf.sepc);
            }
            // Instruction Page Fault (12), Load Page Fault (13), Store Page Fault (15)
            12 | 13 | 15 => {
                let from_user = (tf.sstatus & (1 << 8)) == 0;
                println!(
                    "[TRAP:PAGE_FAULT] mode={} addr=0x{:016x} sepc=0x{:016x} cause={}",
                    if from_user { "U" } else { "S" },
                    tf.stval,
                    tf.sepc,
                    code
                );
                if !from_user {
                    panic!("Supervisor page fault at sepc=0x{:016x} stval=0x{:016x} cause={}", tf.sepc, tf.stval, code);
                }
                tf.regs[10] = (-14isize) as usize; // -EFAULT
                let exited = if let Some(mut sched) = crate::task::SCHEDULER.try_lock() {
                    sched.exit_current()
                } else {
                    false
                };
                println!("[TRAP] Faulted userspace task marked exited: {}", exited);
                if unsafe { IN_LIFECYCLE_TEST } {
                    LAST_USER_FAULT_CODE.store(-11, core::sync::atomic::Ordering::Release);
                    unsafe {
                        return_to_kernel_mode();
                    }
                }
                unsafe { core::arch::asm!("csrsi sstatus, 2", options(nomem, nostack)); }
                loop {
                    crate::task::yield_now();
                    unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
                }
            }
            // Illegal instruction
            2 => {
                let from_user = (tf.sstatus & (1 << 8)) == 0;
                println!(
                    "[TRAP:ILLEGAL_INSN] mode={} sepc=0x{:016x} val=0x{:016x}",
                    if from_user { "U" } else { "S" },
                    tf.sepc,
                    tf.stval
                );
                if !from_user {
                    panic!("Supervisor illegal instruction at sepc=0x{:016x}", tf.sepc);
                }
                let exited = if let Some(mut sched) = crate::task::SCHEDULER.try_lock() {
                    sched.exit_current()
                } else {
                    false
                };
                println!("[TRAP] Illegal-instruction task marked exited: {}", exited);
                if unsafe { IN_LIFECYCLE_TEST } {
                    LAST_USER_FAULT_CODE.store(-4, core::sync::atomic::Ordering::Release);
                    unsafe {
                        return_to_kernel_mode();
                    }
                }
                unsafe { core::arch::asm!("csrsi sstatus, 2", options(nomem, nostack)); }
                loop {
                    crate::task::yield_now();
                    unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
                }
            }
            _ => {
                let from_user = (tf.sstatus & (1 << 8)) == 0;
                println!(
                    "[TRAP:EXCEPTION] mode={} scause={} sepc=0x{:016x} stval=0x{:016x}",
                    if from_user { "U" } else { "S" },
                    code,
                    tf.sepc,
                    tf.stval
                );
                if !from_user {
                    panic!("Unhandled supervisor exception scause={} at sepc=0x{:016x}", code, tf.sepc);
                }
                if let Some(mut sched) = crate::task::SCHEDULER.try_lock() {
                    let _ = sched.exit_current();
                }
                if unsafe { IN_LIFECYCLE_TEST } {
                    LAST_USER_FAULT_CODE.store(-1, core::sync::atomic::Ordering::Release);
                    unsafe {
                        return_to_kernel_mode();
                    }
                }
                unsafe { core::arch::asm!("csrsi sstatus, 2", options(nomem, nostack)); }
                loop {
                    crate::task::yield_now();
                    unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
                }
            }
        }
    }
}

/// Returns the top address of the primordial kernel trap stack.
pub fn kernel_stack_top() -> usize {
    (&raw const TRAP_KERNEL_STACK) as usize + 16384
}

/// Drops CPU privilege from S-mode to U-mode with user arguments in a0 and a1.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user_mode_with_args(
    entry_point: usize,
    user_sp: usize,
    kstack_top: usize,
    arg0: usize,
    arg1: usize,
) -> ! {
    naked_asm!(
        // a0 = entry_point, a1 = user_sp, a2 = kstack_top, a3 = arg0, a4 = arg1
        // 1. Clear SIE, SPP (bit 8), SUM (bit 18), and VS (bits [10:9]), set SPIE (bit 5) and
        // FS = Initial (0b01, bit 13) BEFORE arming sscratch so an S-mode interrupt cannot land
        // between csrw sscratch and sret, U-mode cannot fault on FP or inherit VS, and
        // subsequent S-mode context switches never trap on fsd/fld with FS=Off.
        "csrr t0, sstatus",
        "li t1, ~((1 << 8) | (1 << 18))",
        "and t0, t0, t1",
        "li t1, ~((1 << 1) | (3 << 9) | (3 << 13))",
        "and t0, t0, t1",
        "li t1, (1 << 5) | (1 << 13)",
        "or t0, t0, t1",
        "csrw sstatus, t0",
        // Sanitize all FP registers and fcsr so U-mode never observes kernel/prior FP state,
        // then reset FS back to Initial (0b01).
        "fscsr zero",
        "fmv.d.x f0, zero", "fmv.d.x f1, zero", "fmv.d.x f2, zero", "fmv.d.x f3, zero",
        "fmv.d.x f4, zero", "fmv.d.x f5, zero", "fmv.d.x f6, zero", "fmv.d.x f7, zero",
        "fmv.d.x f8, zero", "fmv.d.x f9, zero", "fmv.d.x f10, zero", "fmv.d.x f11, zero",
        "fmv.d.x f12, zero", "fmv.d.x f13, zero", "fmv.d.x f14, zero", "fmv.d.x f15, zero",
        "fmv.d.x f16, zero", "fmv.d.x f17, zero", "fmv.d.x f18, zero", "fmv.d.x f19, zero",
        "fmv.d.x f20, zero", "fmv.d.x f21, zero", "fmv.d.x f22, zero", "fmv.d.x f23, zero",
        "fmv.d.x f24, zero", "fmv.d.x f25, zero", "fmv.d.x f26, zero", "fmv.d.x f27, zero",
        "fmv.d.x f28, zero", "fmv.d.x f29, zero", "fmv.d.x f30, zero", "fmv.d.x f31, zero",
        "csrw sstatus, t0",
        // 2. Initialize sscratch with kernel stack for handling traps from U-mode (16-byte aligned)
        "andi a2, a2, -16",
        "csrw sscratch, a2",
        // 3. Set sepc = entry_point (a0)
        "csrw sepc, a0",
        // 4. Set user SP = a1 (enforcing standard 16-byte ABI alignment)
        "andi a1, a1, -16",
        "mv sp, a1",
        // 5. Transfer user arguments and sanitize remaining registers for process boundary isolation
        "mv a0, a3",
        "mv a1, a4",
        "mv a2, zero",
        "mv a3, zero",
        "mv a4, zero",
        "mv a5, zero",
        "mv a6, zero",
        "mv a7, zero",
        "mv t0, zero",
        "mv t1, zero",
        "mv t2, zero",
        "mv t3, zero",
        "mv t4, zero",
        "mv t5, zero",
        "mv t6, zero",
        "mv gp, zero",
        "mv tp, zero",
        // 6. Hardware privilege drop into U-mode at sepc
        "sret",
    );
}

/// Drops CPU execution privilege from S-mode to U-mode via `sret`.
#[unsafe(naked)]
pub unsafe extern "C" fn enter_user_mode(entry_point: usize, user_sp: usize, kstack_top: usize) -> ! {
    naked_asm!(
        "li a3, 0",
        "li a4, 0",
        "j {with_args}",
        with_args = sym enter_user_mode_with_args,
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
pub static LAST_USER_FAULT_CODE: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

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

        // 1. Clear SIE, SPP (bit 8), SUM (bit 18), and VS (bits [10:9]), set SPIE (bit 5) and
        // FS = Initial (0b01, bit 13) BEFORE arming sscratch so an S-mode interrupt cannot land
        // between csrw sscratch and sret, U-mode cannot fault on FP or inherit VS, and
        // subsequent S-mode context switches never trap on fsd/fld with FS=Off.
        "csrr t0, sstatus",
        "li t1, ~((1 << 8) | (1 << 18))",
        "and t0, t0, t1",
        "li t1, ~((1 << 1) | (3 << 9) | (3 << 13))",
        "and t0, t0, t1",
        "li t1, (1 << 5) | (1 << 13)",
        "or t0, t0, t1",
        "csrw sstatus, t0",
        // Sanitize all FP registers and fcsr so U-mode never observes kernel/prior FP state,
        // then reset FS back to Initial (0b01).
        "fscsr zero",
        "fmv.d.x f0, zero", "fmv.d.x f1, zero", "fmv.d.x f2, zero", "fmv.d.x f3, zero",
        "fmv.d.x f4, zero", "fmv.d.x f5, zero", "fmv.d.x f6, zero", "fmv.d.x f7, zero",
        "fmv.d.x f8, zero", "fmv.d.x f9, zero", "fmv.d.x f10, zero", "fmv.d.x f11, zero",
        "fmv.d.x f12, zero", "fmv.d.x f13, zero", "fmv.d.x f14, zero", "fmv.d.x f15, zero",
        "fmv.d.x f16, zero", "fmv.d.x f17, zero", "fmv.d.x f18, zero", "fmv.d.x f19, zero",
        "fmv.d.x f20, zero", "fmv.d.x f21, zero", "fmv.d.x f22, zero", "fmv.d.x f23, zero",
        "fmv.d.x f24, zero", "fmv.d.x f25, zero", "fmv.d.x f26, zero", "fmv.d.x f27, zero",
        "fmv.d.x f28, zero", "fmv.d.x f29, zero", "fmv.d.x f30, zero", "fmv.d.x f31, zero",
        "csrw sstatus, t0",
        // 2. Initialize sscratch with kernel stack for handling traps from U-mode (16-byte aligned)
        "andi a2, a2, -16",
        "csrw sscratch, a2",
        // 3. Set sepc = entry_point (a0)
        "csrw sepc, a0",
        // 4. Set user SP = a1 (enforcing standard 16-byte ABI alignment)
        "andi a1, a1, -16",
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
        "mv t0, zero",
        "mv t1, zero",
        "mv t2, zero",
        "mv t3, zero",
        "mv t4, zero",
        "mv t5, zero",
        "mv t6, zero",
        "mv gp, zero",
        "mv tp, zero",
        // 6. Hardware privilege drop into U-mode at sepc
        "sret",
        ctx = sym KERNEL_SAVED_CTX,
    );
}

/// Restores kernel callee-saved registers and returns to caller of `enter_user_mode_with_return`.
#[unsafe(naked)]
pub unsafe extern "C" fn return_to_kernel_mode() -> ! {
    naked_asm!(
        "csrw sscratch, zero",
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

