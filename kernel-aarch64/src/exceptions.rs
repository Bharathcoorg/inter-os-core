//! AArch64 Architectural Exception Vector Table & Hardware Trap Handling
//!
//! Installs the standard 2048-byte aligned vector table into `VBAR_EL1`.
//! Provides real hardware trap decoding:
//! - Synchronous `svc #0` system call dispatch into `sys_dispatch`
//! - GICv2 IRQ interrupt acknowledgment and EOI
//! - High-precision fault reporting (FAR_EL1, ESR_EL1, ELR_EL1)

use core::arch::global_asm;

#[repr(C)]
pub struct ExceptionFrame {
    pub regs: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
    pub esr: u64,
}

global_asm!(
    r#"
    .macro save_all_regs
        sub sp, sp, #288
        stp x0, x1, [sp, #0]
        stp x2, x3, [sp, #16]
        stp x4, x5, [sp, #32]
        stp x6, x7, [sp, #48]
        stp x8, x9, [sp, #64]
        stp x10, x11, [sp, #80]
        stp x12, x13, [sp, #96]
        stp x14, x15, [sp, #112]
        stp x16, x17, [sp, #128]
        stp x18, x19, [sp, #144]
        stp x20, x21, [sp, #160]
        stp x22, x23, [sp, #176]
        stp x24, x25, [sp, #192]
        stp x26, x27, [sp, #208]
        stp x28, x29, [sp, #224]
        str x30, [sp, #240]

        mrs x21, elr_el1
        mrs x22, spsr_el1
        mrs x23, esr_el1
        stp x21, x22, [sp, #248]
        str x23, [sp, #264]
    .endm

    .macro restore_all_regs
        ldp x21, x22, [sp, #248]
        msr elr_el1, x21
        msr spsr_el1, x22

        ldp x0, x1, [sp, #0]
        ldp x2, x3, [sp, #16]
        ldp x4, x5, [sp, #32]
        ldp x6, x7, [sp, #48]
        ldp x8, x9, [sp, #64]
        ldp x10, x11, [sp, #80]
        ldp x12, x13, [sp, #96]
        ldp x14, x15, [sp, #112]
        ldp x16, x17, [sp, #128]
        ldp x18, x19, [sp, #144]
        ldp x20, x21, [sp, #160]
        ldp x22, x23, [sp, #176]
        ldp x24, x25, [sp, #192]
        ldp x26, x27, [sp, #208]
        ldp x28, x29, [sp, #224]
        ldr x30, [sp, #240]
        add sp, sp, #288
    .endm

    .section .text.vectors, "ax"
    .balign 2048
    .global exception_vector_table
    exception_vector_table:
        // 1. Current EL with SP0
        save_all_regs
        mov x0, sp
        bl handle_sync_el1t
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_irq_el1t
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_fiq
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_serror
        restore_all_regs
        eret
        .balign 128

        // 2. Current EL with SPx
        save_all_regs
        mov x0, sp
        bl handle_sync_el1h
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_irq_el1h
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_fiq
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_serror
        restore_all_regs
        eret
        .balign 128

        // 3. Lower EL using AArch64
        save_all_regs
        mov x0, sp
        bl handle_sync_lower64
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_irq_lower64
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_fiq
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_serror
        restore_all_regs
        eret
        .balign 128

        // 4. Lower EL using AArch32
        save_all_regs
        mov x0, sp
        bl handle_sync_lower32
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_irq_lower32
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_fiq
        restore_all_regs
        eret
        .balign 128

        save_all_regs
        mov x0, sp
        bl handle_serror
        restore_all_regs
        eret
        .balign 128
    "#
);

#[no_mangle]
pub extern "C" fn handle_sync_el1t(frame: &mut ExceptionFrame) {
    handle_sync(frame, "EL1t");
}

#[no_mangle]
pub extern "C" fn handle_sync_el1h(frame: &mut ExceptionFrame) {
    handle_sync(frame, "EL1h");
}

#[no_mangle]
pub extern "C" fn handle_sync_lower64(frame: &mut ExceptionFrame) {
    handle_sync(frame, "Lower64");
}

#[no_mangle]
pub extern "C" fn handle_sync_lower32(frame: &mut ExceptionFrame) {
    handle_sync(frame, "Lower32");
}

fn handle_sync(frame: &mut ExceptionFrame, context_name: &str) {
    let ec = (frame.esr >> 26) & 0x3F;

    if ec == 0x15 {
        // EC 0x15: SVC instruction execution in AArch64 state
        let syscall_nr = frame.regs[8];
        let a0 = frame.regs[0];
        let a1 = frame.regs[1];
        let a2 = frame.regs[2];
        let a3 = frame.regs[3];
        let a4 = frame.regs[4];

        let ret = crate::syscall::sys_dispatch(syscall_nr, a0, a1, a2, a3, a4);
        frame.regs[0] = ret as u64;
    } else {
        let far: u64;
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("mrs {}, far_el1", out(reg) far);
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            far = 0;
        }
        crate::kprintln!();
        crate::kprintln!("============================================================");
        crate::kprintln!("  [AArch64 SYNCHRONOUS EXCEPTION] ({})", context_name);
        crate::kprintln!("  ESR_EL1: 0x{:016X} (EC: 0x{:02X})", frame.esr, ec);
        crate::kprintln!("  ELR_EL1: 0x{:016X}", frame.elr);
        crate::kprintln!("  FAR_EL1: 0x{:016X}", far);
        crate::kprintln!("  SPSR_EL1: 0x{:016X}", frame.spsr);
        crate::kprintln!("============================================================");
        loop {
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("wfe");
            }
        }
    }
}

pub static mut TIMER_INTERRUPT_COUNT: u64 = 0;

#[no_mangle]
pub extern "C" fn handle_irq_el1t(_frame: &mut ExceptionFrame) {
    dispatch_irq();
}

#[no_mangle]
pub extern "C" fn handle_irq_el1h(_frame: &mut ExceptionFrame) {
    dispatch_irq();
}

#[no_mangle]
pub extern "C" fn handle_irq_lower64(_frame: &mut ExceptionFrame) {
    dispatch_irq();
}

#[no_mangle]
pub extern "C" fn handle_irq_lower32(_frame: &mut ExceptionFrame) {
    dispatch_irq();
}

fn dispatch_irq() {
    let gic = crate::gic::Gicv2::new(crate::gic::DEFAULT_GICD_BASE, crate::gic::DEFAULT_GICC_BASE);
    let irq = gic.acknowledge_irq();

    if irq == 27 {
        // PPI 27: ARM Generic Timer Virtual Timer
        unsafe {
            TIMER_INTERRUPT_COUNT += 1;
        }
        let timer = crate::timer::ArmGenericTimer::new();
        // Re-arm timer countdown for 10ms (100 Hz @ 62.5 MHz = 625,000 cycles)
        timer.program_countdown(625_000);
        gic.end_of_interrupt(irq);
    } else if irq < 1020 {
        gic.end_of_interrupt(irq);
    }
}

#[no_mangle]
pub extern "C" fn handle_fiq(_frame: &mut ExceptionFrame) {}

#[no_mangle]
pub extern "C" fn handle_serror(frame: &mut ExceptionFrame) {
    crate::kprintln!("[AArch64 SERROR] ESR_EL1: 0x{:016X}, ELR_EL1: 0x{:016X}", frame.esr, frame.elr);
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}

pub fn init_exceptions() {
    extern "C" {
        static exception_vector_table: u8;
    }
    unsafe {
        let vbar = &exception_vector_table as *const u8 as u64;
        core::arch::asm!("msr vbar_el1, {}", in(reg) vbar);
        core::arch::asm!("isb");
    }
}

#[inline(always)]
pub unsafe fn svc_call(syscall_nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    #[cfg(target_arch = "aarch64")]
    core::arch::asm!(
        "svc #0",
        inout("x0") a0 => ret,
        in("x1") a1,
        in("x2") a2,
        in("x3") a3,
        in("x8") syscall_nr,
    );
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (syscall_nr, a0, a1, a2, a3);
        ret = 0;
    }
    ret
}
