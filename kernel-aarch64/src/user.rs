// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Ring 3 / EL0 Userspace Execution & Privilege Separation
//!
//! Provides the real hardware privilege drop from EL1 (Kernel) to EL0 (User),
//! managing isolated translation tables, dedicated user stacks, and hardware `svc #0` returns.

use core::arch::global_asm;

pub const USER_CODE_VADDR: u64 = 0x0040_0000;
pub const USER_CODE_PADDR: u64 = 0x4400_0000;
pub const USER_STACK_VADDR: u64 = 0x0080_0000;
pub const USER_STACK_PADDR: u64 = 0x4420_0000;
pub const USER_STACK_SIZE: u64 = 16384;

#[no_mangle]
pub static mut KERNEL_SAVED_CONTEXT: crate::task::ArmContext = crate::task::ArmContext {
    x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0,
    x25: 0, x26: 0, x27: 0, x28: 0, fp: 0, lr: 0, sp: 0,
};

pub static mut EL0_HARDWARE_ENTERED: bool = false;
pub static mut EL0_TICKS_CAPTURED: u64 = 0;
pub static mut EL0_TASK_ID_CAPTURED: u64 = 0;
pub static mut EL0_PRIVILEGE_TRAP_VERIFIED: bool = false;
pub static mut EL0_MMU_TRAP_VERIFIED: bool = false;
pub static mut EL0_EXIT_CODE: i32 = 0;
pub static mut EL0_TEST_COMPLETED: bool = false;
pub static mut IN_SECTION17_SMOKE: bool = true;
pub static mut IN_LIFECYCLE_TEST: bool = false;

extern "C" {
    pub fn enter_user_mode(entry_point: u64, user_sp: u64);
    pub fn return_to_kernel_mode() -> !;
    pub static el0_payload_start: u8;
    pub static el0_payload_end: u8;
}

global_asm!(
    r#"
    .section .text.user_switch, "ax"
    .global enter_user_mode
    enter_user_mode:
        // x0 = entry_point, x1 = user_sp
        // 1. Save kernel callee-saved registers and SP into KERNEL_SAVED_CONTEXT
        adrp x2, KERNEL_SAVED_CONTEXT
        add x2, x2, :lo12:KERNEL_SAVED_CONTEXT
        stp x19, x20, [x2, #0]
        stp x21, x22, [x2, #16]
        stp x23, x24, [x2, #32]
        stp x25, x26, [x2, #48]
        stp x27, x28, [x2, #64]
        stp x29, x30, [x2, #80]
        mov x3, sp
        str x3, [x2, #96]

        // 2. Set up user stack pointer in SP_EL0
        msr sp_el0, x1

        // 3. Set up user return address in ELR_EL1
        msr elr_el1, x0

        // 4. Set up SPSR_EL1:
        // M[3:0] = 0b0000 (EL0t)
        // M[4]   = 0 (AArch64 execution state)
        // DAIF[9:6] = 0b1111 (0x3c0: Debug, SError, IRQ, FIQ masked during test)
        mov x4, #0x3c0
        msr spsr_el1, x4

        // 5. Real hardware privilege transition from EL1 to EL0
        eret

    .global return_to_kernel_mode
    return_to_kernel_mode:
        // Restore kernel callee-saved registers and SP from KERNEL_SAVED_CONTEXT
        adrp x2, KERNEL_SAVED_CONTEXT
        add x2, x2, :lo12:KERNEL_SAVED_CONTEXT
        ldp x19, x20, [x2, #0]
        ldp x21, x22, [x2, #16]
        ldp x23, x24, [x2, #32]
        ldp x25, x26, [x2, #48]
        ldp x27, x28, [x2, #64]
        ldp x29, x30, [x2, #80]
        ldr x3, [x2, #96]
        mov sp, x3
        ret

    .section .text.user_payload, "ax"
    .balign 8
    .global el0_payload_start
    .global el0_payload_end
    el0_payload_start:
        // --- 1. Real Native EL0 Syscall: SYS_GET_TICKS (syscall 4) ---
        mov x8, #4
        svc #0
        mov x19, x0

        // --- 2. Real Native EL0 Syscall: SYS_GET_TASK_ID (syscall 7) ---
        mov x8, #7
        svc #0
        mov x20, x0

        // --- 3. Negative Invariant: Privilege Isolation ---
        // Attempting to read CurrentEL from EL0 is forbidden.
        // It must trap to EL1 exception vector with EC=0x00.
        mrs x21, CurrentEL

        // --- 4. Negative Invariant: MMU Memory Isolation ---
        // Attempting to read Kernel DRAM (0x40000000) from EL0 is forbidden by DESC_AP_RW_EL1.
        // It must trap to EL1 exception vector with Data Abort (EC=0x24).
        movz x22, #0x4000, lsl #16
        ldr x23, [x22]

        // --- 5. Clean Exit Syscall: SYS_EXIT (syscall 2) with exit code 42 ---
        mov x8, #2
        mov x0, #42
        svc #0

        // In case of unexpected return, loop
    1:  wfe
        b 1b
    el0_payload_end:
    "#
);

pub struct UserProcess {
    pub entry_point: u64,
    pub stack_top: u64,
}

impl UserProcess {
    pub fn new(entry_point: u64, stack_top: u64) -> Self {
        Self { entry_point, stack_top }
    }

    pub unsafe fn load_code(&self, code: &[u8]) {
        let dest = USER_CODE_PADDR as *mut u8;
        core::ptr::copy_nonoverlapping(code.as_ptr(), dest, code.len());
        #[cfg(target_arch = "aarch64")]
        {
            core::arch::asm!(
                "dc cvau, {0}",
                "dsb ish",
                "ic ivau, {0}",
                "dsb ish",
                "isb",
                in(reg) USER_CODE_VADDR
            );
        }
    }
}

pub fn get_el0_payload() -> &'static [u8] {
    unsafe {
        let start = &el0_payload_start as *const u8;
        let end = &el0_payload_end as *const u8;
        let len = end as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    }
}

pub fn record_el0_exit(exit_code: i32) {
    unsafe {
        EL0_EXIT_CODE = exit_code;
        EL0_TEST_COMPLETED = true;
    }
}
