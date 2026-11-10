// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! x86_64 Long Mode Task Context & Context Switching
//!
//! Provides the low-level hardware register state saving and restoring
//! mechanism for cooperative and preemptive multitasking.

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
    pub fxsave_area: [u8; 512],
}

impl TaskContext {
    /// Creates an empty context with null stack pointer and zeroed FPU state.
    pub const fn empty() -> Self {
        Self {
            rsp: 0,
            _pad: 0,
            fxsave_area: [0; 512],
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

        let mut fx = [0u8; 512];
        // Standard initial x87/SSE configuration:
        // FCW = 0x037F (all FPU exceptions masked, double extended precision, round to nearest)
        fx[0] = 0x7F;
        fx[1] = 0x03;
        // MXCSR = 0x1F80 (all SSE exceptions masked, round to nearest)
        fx[24] = 0x80;
        fx[25] = 0x1F;

        Self {
            rsp: frame as usize as u64,
            _pad: 0,
            fxsave_area: fx,
        }
    }
}

/// Performs a low-level hardware context switch between two tasks.
///
/// Saves callee-saved registers, RFLAGS, and FPU/SSE state of the current task,
/// stores the current RSP into `*prev`, then loads RSP and restores FPU/SSE and
/// registers from `*next`, and returns (`ret`) into the new task.
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

        // Return into next task (pops instruction pointer from stack)
        "ret"
    );
}
