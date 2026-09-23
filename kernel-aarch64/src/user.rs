//! AArch64 Ring 3 / EL0 Userspace Execution & Privilege Separation
//!
//! Provides the hardware privilege drop from EL1 (Kernel) to EL0 (User),
//! managing isolated address spaces, dedicated user stacks, and hardware `svc #0` returns.

pub const USER_CODE_VADDR: u64 = 0x0040_0000;
pub const USER_CODE_PADDR: u64 = 0x4400_0000;
pub const USER_STACK_VADDR: u64 = 0x0080_0000;
pub const USER_STACK_PADDR: u64 = 0x4420_0000;
pub const USER_STACK_SIZE: u64 = 16384;

pub static mut EL0_TEST_COMPLETED: bool = false;
pub static mut EL0_RECEIVED_TICKS: u64 = 0;
pub static mut EL0_EXIT_CODE: i32 = 0;

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
    }
}

/// Assembles a bare-metal EL0 test routine in native ARM64 machine code:
/// 1. mov x8, #4 (SYS_GET_TICKS)
/// 2. svc #0
/// 3. mov x19, x0 (save ticks)
/// 4. mov x8, #7 (SYS_GET_TASK_ID)
/// 5. svc #0
/// 6. mov x8, #2 (SYS_EXIT)
/// 7. mov x0, #42 (exit code 42)
/// 8. svc #0
/// 9. b . (infinite halt)
pub fn generate_el0_test_payload() -> [u32; 10] {
    [
        0xd2800088, // mov x8, #4 (SYS_GET_TICKS)
        0xd4000001, // svc #0
        0xaa0003f3, // mov x19, x0 (save returned ticks into x19)
        0xd28000e8, // mov x8, #7 (SYS_GET_TASK_ID)
        0xd4000001, // svc #0
        0xd2800048, // mov x8, #2 (SYS_EXIT)
        0xd2800540, // mov x0, #42 (exit code 42)
        0xd4000001, // svc #0
        0x14000000, // b . (loop)
        0xd503201f, // nop
    ]
}

pub fn record_el0_exit(exit_code: i32) {
    unsafe {
        EL0_EXIT_CODE = exit_code;
        EL0_TEST_COMPLETED = true;
    }
}
