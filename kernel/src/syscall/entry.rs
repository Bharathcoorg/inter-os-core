//! x86_64 Fast System Call Entry & MSR Configuration
//!
//! Configures CPU Model-Specific Registers (MSRs) for hardware `syscall` / `sysretq`
//! execution, providing sub-microsecond user-mode to kernel-mode privilege transitions.

use crate::serial_println;

/// Model-Specific Register addresses for AMD64 / Intel 64 fast system calls.
pub mod msr {
    /// Extended Feature Enable Register (bit 0 = SCE: System Call Enable).
    pub const IA32_EFER: u32 = 0xC000_0080;
    /// Target CS and SS selectors for SYSCALL and SYSRET.
    pub const IA32_STAR: u32 = 0xC000_0081;
    /// Target 64-bit RIP for SYSCALL in 64-bit Long Mode.
    pub const IA32_LSTAR: u32 = 0xC000_0082;
    /// RFLAGS mask applied during SYSCALL entry (clears specified flags).
    pub const IA32_FMASK: u32 = 0xC000_0084;
}

/// Bitmask in `IA32_EFER` enabling the `syscall` and `sysretq` instructions.
pub const EFER_SCE_BIT: u64 = 1 << 0;

/// RFLAGS bits cleared automatically on `syscall` entry:
/// - Bit 9: Interrupt Enable Flag (IF) -> enters kernel with interrupts disabled.
/// - Bit 10: Direction Flag (DF) -> clears string direction to forward.
/// - Bit 8: Trap Flag (TF) -> disables single-stepping.
pub const FMASK_SYSCALL: u64 = 0x200 | 0x400 | 0x100;

/// Scratch storage for user stack pointer during kernel entry.
#[no_mangle]
static mut USER_RSP_SCRATCH: u64 = 0;

/// Dedicated per-task kernel stack top (dynamically loaded on task switch).
#[no_mangle]
pub static mut CURRENT_TASK_KERNEL_RSP: u64 = 0;

/// Dedicated 128 KiB fallback kernel stack used during bootstrap syscall execution.
#[allow(dead_code)]
#[repr(align(16))]
struct SyscallStack([u8; 128 * 1024]);
static mut SYSCALL_KERNEL_STACK: SyscallStack = SyscallStack([0; 128 * 1024]);

/// Updates the current active task's dedicated kernel stack top for syscall entry.
#[inline(always)]
pub fn set_current_kernel_rsp(rsp: u64) {
    unsafe {
        CURRENT_TASK_KERNEL_RSP = rsp;
    }
}

/// Reads a 64-bit value from a Model-Specific Register.
#[inline(always)]
pub unsafe fn rdmsr(msr_id: u32) -> u64 {
    let low: u32;
    let high: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr_id,
        out("eax") low,
        out("edx") high,
        options(nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Writes a 64-bit value to a Model-Specific Register.
#[inline(always)]
pub unsafe fn wrmsr(msr_id: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr_id,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags)
    );
}

/// Initializes hardware MSRs for fast system calls.
pub fn init() {
    unsafe {
        // 1. Enable System Call Extension (SCE) in IA32_EFER
        let efer = rdmsr(msr::IA32_EFER);
        wrmsr(msr::IA32_EFER, efer | EFER_SCE_BIT);

        // 2. Configure IA32_STAR segment selector bases:
        // - Bits 47:32: Kernel CS selector base.
        //   Hardware sets Kernel CS = STAR[47:32] & ~3 (0x08)
        //   Hardware sets Kernel SS = (STAR[47:32] & ~3) + 8 (0x10)
        // - Bits 63:48: User CS base for 64-bit SYSRET.
        //   Hardware sets User SS = (STAR[63:48] + 8) | 3 (0x18 | 3 = 0x1B)
        //   Hardware sets User CS = (STAR[63:48] + 16) | 3 (0x20 | 3 = 0x23)
        let kernel_cs_base = 0x08u64;
        let user_cs_base = 0x10u64;
        let star_value = (user_cs_base << 48) | (kernel_cs_base << 32);
        wrmsr(msr::IA32_STAR, star_value);

        // 3. Configure IA32_LSTAR with address of naked syscall entry stub
        let entry_addr = syscall_entry as *const () as usize as u64;
        wrmsr(msr::IA32_LSTAR, entry_addr);

        // 4. Configure IA32_FMASK to clear IF, DF, and TF
        wrmsr(msr::IA32_FMASK, FMASK_SYSCALL);

        // 5. Hardware MSR read-back verification
        let read_lstar = rdmsr(msr::IA32_LSTAR);
        let read_star = rdmsr(msr::IA32_STAR);
        let read_fmask = rdmsr(msr::IA32_FMASK);
        let read_efer = rdmsr(msr::IA32_EFER);

        assert_eq!(read_lstar, entry_addr, "LSTAR MSR mismatch");
        assert_eq!(read_star, star_value, "STAR MSR mismatch");
        assert_eq!(read_fmask, FMASK_SYSCALL, "FMASK MSR mismatch");
        assert_ne!(read_efer & EFER_SCE_BIT, 0, "EFER SCE bit not set");
    }

    serial_println!("[OK] Hardware MSR fast system call extensions enabled and verified (SCE, STAR, LSTAR, FMASK).");
}

/// Low-level naked hardware entry stub invoked by CPU on `syscall`.
///
/// Hardware sets:
/// - `rcx`: user return instruction pointer (RIP)
/// - `r11`: user CPU flags (RFLAGS)
/// - `cs`, `ss`: kernel segment selectors
/// - `rflags`: masked by `IA32_FMASK`
/// - `rsp`: remains pointing to user stack!
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // 1. Save user stack pointer to scratch variable
        "mov [rip + {user_rsp}], rsp",

        // 2. Switch to dedicated per-task kernel stack (or static fallback if 0)
        "mov rsp, [rip + {current_kernel_rsp}]",
        "test rsp, rsp",
        "jnz 2f",
        "lea rsp, [rip + {kernel_stack} + 131072]",
        "2:",

        // 3. Push user execution context onto kernel stack
        "push qword ptr [rip + {user_rsp}]", // User RSP
        "push r11",                          // User RFLAGS
        "push rcx",                          // User RIP
        "push rbp",                          // Callee-saved RBP
        "push rbx",                          // Callee-saved RBX
        "push r12",                          // Callee-saved R12
        "push r13",                          // Callee-saved R13
        "push r14",                          // Callee-saved R14
        "push r15",                          // Callee-saved R15

        // 4. Map user syscall register calling convention to Rust dispatch function:
        // User registers:
        //   rax = syscall_number
        //   rdi = arg1
        //   rsi = arg2
        //   rdx = arg3
        //   r10 = arg4 (rcx was clobbered by CPU hardware)
        //   r8  = arg5
        //   r9  = arg6
        //
        // System V AMD64 ABI function call convention:
        //   rdi = 1st arg (syscall_number)
        //   rsi = 2nd arg (arg1)
        //   rdx = 3rd arg (arg2)
        //   rcx = 4th arg (arg3)
        //   r8  = 5th arg (arg4)
        //   r9  = 6th arg (arg5)
        "mov r9, r8",   // arg5 -> r9
        "mov r8, r10",  // arg4 -> r8
        "mov rcx, rdx", // arg3 -> rcx
        "mov rdx, rsi", // arg2 -> rdx
        "mov rsi, rdi", // arg1 -> rsi
        "mov rdi, rax", // syscall_number -> rdi

        // 5. Call Rust unified system call dispatcher
        "call {sys_dispatch}",

        // Return value is in RAX (preserved as syscall return code)

        // 6. Restore callee-saved registers from kernel stack
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",

        // 7. Restore user return instruction pointer and flags
        "pop rcx", // User RIP restored to RCX for sysretq
        "pop r11", // User RFLAGS restored to R11 for sysretq

        // 8. Restore user stack pointer
        "pop rsp",

        // 9. Fast return to User Mode (Ring 3)
        // Hardware sets:
        // - rip <- rcx
        // - rflags <- r11
        // - cs <- User 64-bit Code Selector (0x23)
        // - ss <- User Data Selector (0x1B)
        "sysretq",

        user_rsp = sym USER_RSP_SCRATCH,
        current_kernel_rsp = sym CURRENT_TASK_KERNEL_RSP,
        kernel_stack = sym SYSCALL_KERNEL_STACK,
        sys_dispatch = sym crate::syscall::sys_dispatch,
    );
}

/// Executes a raw hardware fast system call (`syscall` instruction).
///
/// Can be invoked by Ring 3 user tasks or for hardware self-tests.
#[inline(always)]
pub unsafe fn raw_syscall(
    num: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") num => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        in("r10") a4,
        in("r8") a5,
        out("rcx") _,
        out("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}
