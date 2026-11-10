// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! x86_64 Long Mode Task Context & Context Switching
//!
//! Provides the low-level hardware register state saving and restoring
//! mechanism for cooperative and preemptive multitasking.

/// 512-byte FPU/SSE legacy/streaming hardware state buffer guaranteed to have 16-byte alignment.
///
/// Hardware layout saved and restored by `FXSAVE64` and `FXRSTOR64`:
/// - Bytes 0..31: x87 FPU environment (FCW, FSW, FTW, FOP, FIP, FDP).
/// - Bytes 32..159: x87 / MMX registers ST0/MM0 .. ST7/MM7 (8 x 16-byte slots, 10 bytes used each).
/// - Bytes 160..287: SSE XMM0 .. XMM7 registers (8 x 16 bytes).
/// - Bytes 288..415: SSE XMM8 .. XMM15 registers (8 x 16 bytes in 64-bit mode).
/// - Bytes 416..511: Reserved / implementation specific.
///
/// Note on AVX / AVX-512: AVX upper halves (YMM0..YMM15 upper 128 bits) and AVX-512 (ZMM registers)
/// require `XSAVE`/`XRSTOR` and OS management via `XCR0` and `CR4.OSXSAVE`. InterOS currently operates
/// with `CR4.OSXSAVE` disabled, strictly supporting 128-bit SSE/SSE2/SSE3/SSSE3/SSE4. AVX/AVX-512 are
/// disabled at the CPU level (#UD on attempt).
///
/// Intel SDM Vol 2A: `FXSAVE` and `FXRSTOR` require memory operands to be aligned
/// on a 16-byte boundary; otherwise a General Protection exception (`#GP(0)`) is raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(16))]
pub struct FpuState(pub [u8; 512]);

impl FpuState {
    /// Creates an FPU state initialized with standard masked exception control words.
    ///
    /// - FCW (bytes 0..2) = 0x037F: all x87 exceptions masked, 64-bit precision, round to nearest.
    /// - MXCSR (bytes 24..28) = 0x1F80: all SSE exceptions masked, round to nearest.
    pub const fn empty() -> Self {
        let mut data = [0u8; 512];
        data[0] = 0x7F;
        data[1] = 0x03;
        data[24] = 0x80;
        data[25] = 0x1F;
        Self(data)
    }
}

/// CPU execution context holding the kernel stack pointer and FPU/SIMD state.
///
/// Callee-saved registers (`rbx`, `rbp`, `r12`, `r13`, `r14`, `r15`, `rflags`)
/// and the instruction pointer (`rip`) are saved directly on the task's stack.
/// Floating-point, MMX, and SSE (`xmm0..xmm15`) registers are saved to `fxsave_area`.
#[derive(Debug, Clone, Copy)]
#[repr(C, align(16))]
pub struct TaskContext {
    /// Saved kernel stack pointer pointing to the top of the preserved register frame.
    pub rsp: u64,
    /// 16-byte alignment padding.
    pub _pad: u64,
    /// 512-byte FXSAVE/FXRSTOR hardware state buffer (offset 16, 16-byte aligned).
    pub fxsave_area: FpuState,
}

// Compile-time verification of struct layout requirements for FXSAVE64/FXRSTOR64
const _: () = {
    assert!(core::mem::align_of::<TaskContext>() == 16);
    assert!(core::mem::align_of::<FpuState>() == 16);
    assert!(core::mem::offset_of!(TaskContext, fxsave_area) == 16);
    assert!(core::mem::size_of::<TaskContext>() % 16 == 0);
};

impl TaskContext {
    /// Creates an empty context with null stack pointer and sanitized initial FPU state.
    pub const fn empty() -> Self {
        Self {
            rsp: 0,
            _pad: 0,
            fxsave_area: FpuState::empty(),
        }
    }

    /// Initializes a task's stack frame so that a context switch to this context
    /// pops all registers and executes `entry_point`.
    ///
    /// The stack memory buffer must remain valid for the duration of the task.
    pub unsafe fn create_initial(stack_slice: &mut [u8], entry_point: extern "C" fn() -> !) -> Self {
        let stack_top = stack_slice.as_ptr() as usize + stack_slice.len();

        // 16-byte align the top of stack
        let aligned_top = (stack_top & !0xF) as *mut u64;

        // AMD64 System V ABI requirement:
        // When control transfers to a function entry point, (rsp + 8) % 16 == 0
        // (because a normal `call` instruction pushes the 8-byte return address).
        // If we jump/ret directly with rsp % 16 == 0, compiler prologues expecting
        // (rsp + 8) % 16 == 0 will result in misaligned stack offsets for SSE / local variables.
        //
        // Frame layout below aligned_top:
        // [aligned_top - 1] = 0 (dummy return address; after `ret`, RSP will point here, so rsp % 16 == 8)
        // [aligned_top - 2] = entry_point (popped by `switch_context`'s `ret`)
        // [aligned_top - 3] = rbx (0)
        // [aligned_top - 4] = rbp (0)
        // [aligned_top - 5] = r12 (0)
        // [aligned_top - 6] = r13 (0)
        // [aligned_top - 7] = r14 (0)
        // [aligned_top - 8] = r15 (0)
        // [aligned_top - 9] = rflags (0x0202: Interrupts Enabled)

        let frame = aligned_top.sub(9);
        *aligned_top.sub(1) = 0;                      // Dummy return slot
        *frame.add(7) = entry_point as usize as u64; // Return address for `ret`
        *frame.add(6) = 0;                          // rbx
        *frame.add(5) = 0;                          // rbp
        *frame.add(4) = 0;                          // r12
        *frame.add(3) = 0;                          // r13
        *frame.add(2) = 0;                          // r14
        *frame.add(1) = 0;                          // r15
        *frame.add(0) = 0x0202;                     // rflags (IF = 1)

        Self {
            rsp: frame as usize as u64,
            _pad: 0,
            fxsave_area: FpuState::empty(),
        }
    }
}

/// Returns whether the CPU reports support for FXSAVE/FXRSTOR via CPUID.1.EDX[bit 24].
pub fn cpu_supports_fxsave() -> bool {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    unsafe {
        let edx: u32;
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            lateout("edx") edx,
            out("eax") _,
            out("ecx") _,
            options(nomem, nostack)
        );
        (edx & (1 << 24)) != 0
    }
    #[cfg(any(not(target_arch = "x86_64"), test))]
    {
        true
    }
}

/// Ensures CPU CR4 and CR0 control registers have FXSAVE/FXRSTOR and SSE enabled.
///
/// Invariants:
/// - CR0.EM (bit 2) is cleared (FPU emulation off; native FPU/SSE execution allowed).
/// - CR0.TS (bit 3) is cleared (task switch flag off).
/// - CR0.MP (bit 1) is set (monitor coprocessor).
/// - CR0.NE (bit 5) is set (native numeric exceptions via #MF rather than legacy IRQ13).
/// - CR4.OSFXSR (bit 9) is set (enables FXSAVE/FXRSTOR instructions and 128-bit XMM).
/// - CR4.OSXMMEXCPT (bit 10) is set (unmasked SIMD exceptions trigger #XM exception).
/// - CR4.OSXSAVE (bit 18) remains cleared (AVX/XSAVE disabled).
pub fn init_fpu() {
    #[cfg(all(target_arch = "x86_64", not(test)))]
    unsafe {
        if !cpu_supports_fxsave() {
            crate::serial_println!("[FPU] FATAL: CPU does not support FXSAVE/FXRSTOR (CPUID.1.EDX[24] = 0)");
            return;
        }

        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        // Clear EM (bit 2) and TS (bit 3), set MP (bit 1) and NE (bit 5)
        cr0 &= !((1u64 << 2) | (1u64 << 3));
        cr0 |= (1u64 << 1) | (1u64 << 5);
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));

        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        // Set OSFXSR (bit 9) and OSXMMEXCPT (bit 10), ensure OSXSAVE (bit 18) is cleared
        cr4 |= (1u64 << 9) | (1u64 << 10);
        cr4 &= !(1u64 << 18);
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }
}

/// Performs a low-level hardware context switch between two tasks.
///
/// Saves callee-saved registers, RFLAGS, and FPU/SSE state of the current task,
/// stores the current RSP into `*prev`, then loads RSP and restores FPU/SSE and
/// registers from `*next`, sanitizes scratch registers to prevent information leakage,
/// and returns (`ret`) into the new task.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(prev: *mut TaskContext, next: *const TaskContext) {
    core::arch::naked_asm!(
        // Save current task's callee-saved registers and flags to current stack
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "pushfq",

        // Store current stack pointer to prev.rsp
        "mov [rdi], rsp",

        // Save current task's FPU / SSE / XMM state to prev.fxsave_area (offset 16)
        "fxsave64 [rdi + 16]",

        // Load new stack pointer from next.rsp
        "mov rsp, [rsi]",

        // Restore new task's FPU / SSE / XMM state from next.fxsave_area (offset 16)
        "fxrstor64 [rsi + 16]",

        // Restore new task's flags and callee-saved registers from new stack
        "popfq",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",

        // Sanitize scratch / caller-saved registers to prevent kernel pointer leakage
        // (prev and next in rdi/rsi) and cross-task data remanence
        "xor eax, eax",
        "xor ecx, ecx",
        "xor edx, edx",
        "xor esi, esi",
        "xor edi, edi",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",

        // Return into next task (pops instruction pointer from stack)
        "ret"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_context_and_fpu_state_alignment() {
        assert_eq!(core::mem::align_of::<TaskContext>(), 16);
        assert_eq!(core::mem::align_of::<FpuState>(), 16);
        assert_eq!(core::mem::offset_of!(TaskContext, fxsave_area), 16);
        assert_eq!(core::mem::size_of::<TaskContext>() % 16, 0);

        let ctx = TaskContext::empty();
        assert_eq!(ctx.fxsave_area.0[0], 0x7F);
        assert_eq!(ctx.fxsave_area.0[1], 0x03);
        assert_eq!(ctx.fxsave_area.0[24], 0x80);
        assert_eq!(ctx.fxsave_area.0[25], 0x1F);
    }
}
