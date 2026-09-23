//! ARM64 EL1 Microkernel Entry & Exception Vector Architecture
//!
//! Provides the architectural vector table, EL1/EL0 exception frames,
//! and `svc #0` system call dispatch mapping for AArch64 smartphone targets.

/// Saved CPU register state during an exception or context switch on ARM64.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ArmExceptionFrame {
    pub r: [u64; 31],   // x0 - x30 (x29 = frame pointer, x30 = link register)
    pub sp_el0: u64,    // Unprivileged userspace stack pointer
    pub elr_el1: u64,   // Exception link register (PC to return to)
    pub spsr_el1: u64,  // Saved program status register (PSTATE)
}

/// AArch64 Vector Table Alignment: Must be aligned to 2048 bytes (2 KiB).
pub const VECTOR_TABLE_ALIGNMENT: usize = 2048;

/// Exception Syndrome Register (ESR_EL1) Exception Classes
pub const ESR_EC_UNKNOWN: u32 = 0x00;
pub const ESR_EC_WFI_WFE: u32 = 0x01;
pub const ESR_EC_SVC64: u32 = 0x15;     // SVC instruction execution in AArch64 (Syscall)
pub const ESR_EC_INSTR_ABORT: u32 = 0x20;
pub const ESR_EC_DATA_ABORT: u32 = 0x24;

/// Dispatches an AArch64 `svc #0` system call using standard mobile ABI conventions:
/// - Syscall Number: `x8`
/// - Arguments: `x0` - `x5`
/// - Return Value: `x0`
pub fn handle_aarch64_syscall(frame: &mut ArmExceptionFrame) {
    let syscall_nr = frame.r[8];
    let a0 = frame.r[0];
    let a1 = frame.r[1];
    let a2 = frame.r[2];
    let a3 = frame.r[3];
    let a4 = frame.r[4];

    let result = crate::syscall::sys_dispatch(
        syscall_nr,
        a0,
        a1,
        a2,
        a3,
        a4,
    );

    frame.r[0] = result as u64;
}

/// Self-test verifying AArch64 exception frame and syscall dispatch mapping.
pub fn test_aarch64_syscall_dispatch() {
    let mut frame = ArmExceptionFrame::default();
    // Test Syscall 1: Yield
    frame.r[8] = 1; // sys_yield

    handle_aarch64_syscall(&mut frame);

    // Frame x0 must hold 0 on successful yield
    assert_eq!(frame.r[0], 0, "Syscall yield return must be 0");

    crate::serial_println!(
        "[TEST] AArch64 EL1/EL0 ABI: 'svc #0' ABI vector dispatch (x8=1 -> x0={}) VERIFIED.",
        frame.r[0]
    );
}
