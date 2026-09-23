//! x86_64 Long Mode Task Context & Context Switching
//!
//! Provides the low-level hardware register state saving and restoring
//! mechanism for cooperative and preemptive multitasking.

/// CPU execution context holding the kernel stack pointer.
///
/// Callee-saved registers (`rbx`, `rbp`, `r12`, `r13`, `r14`, `r15`, `rflags`)
/// and the instruction pointer (`rip`) are saved directly on the task's stack.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TaskContext {
    /// Saved kernel stack pointer pointing to the top of the preserved register frame.
    pub rsp: u64,
}

impl TaskContext {
    /// Creates an empty context with null stack pointer.
    pub const fn empty() -> Self {
        Self { rsp: 0 }
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
        }
    }
}

/// Performs a low-level hardware context switch between two tasks.
///
/// Saves callee-saved registers and RFLAGS of the current task to its stack,
/// stores the current RSP into `*prev`, then loads RSP from `*next`, restores
/// all registers, and returns (`ret`) into the new task.
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

        // Load new stack pointer from next.rsp
        "mov rsp, [rsi]",

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
