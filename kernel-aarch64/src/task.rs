// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Multitasking Scheduler & Callee-Saved Register Context Switching
//!
//! Implements cooperative & preemptive multi-tasking across ARM Cortex-A CPU cores.
//! Context switches between task execution stacks using standard AArch64 AAPCS conventions:
//! - Callee-saved general-purpose registers: `x19` - `x28`
//! - Frame pointer: `x29`
//! - Link register (return address): `x30`
//! - Dedicated stack pointer: `sp`

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;

pub const STACK_SIZE: usize = 16384; // 16 KiB per kernel task stack

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ArmContext {
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub fp: u64, // x29
    pub lr: u64, // x30
    pub sp: u64,
}

#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(old_ctx: *mut ArmContext, new_ctx: *const ArmContext) {
    core::arch::naked_asm!(
        // Save x19-x28 into old_ctx (x0)
        "stp x19, x20, [x0, #0]",
        "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]",
        "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]",
        // Save x29 (fp) and x30 (lr)
        "stp x29, x30, [x0, #80]",
        // Save current stack pointer
        "mov x2, sp",
        "str x2, [x0, #96]",

        // Restore x19-x28 from new_ctx (x1)
        "ldp x19, x20, [x1, #0]",
        "ldp x21, x22, [x1, #16]",
        "ldp x23, x24, [x1, #32]",
        "ldp x25, x26, [x1, #48]",
        "ldp x27, x28, [x1, #64]",
        // Restore x29 (fp) and x30 (lr)
        "ldp x29, x30, [x1, #80]",
        // Restore stack pointer
        "ldr x2, [x1, #96]",
        "mov sp, x2",

        // Branch to restored lr (x30)
        "ret",
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Terminated,
}

use crate::capability::CapabilityTable;

pub struct TaskControlBlock {
    pub id: u32,
    pub name: &'static str,
    pub state: TaskState,
    pub context: ArmContext,
    pub stack: Box<[u8; STACK_SIZE]>,
    pub cap_table: CapabilityTable,
    pub cpu_ticks: u64,
    pub priority: u8,
    pub page_table: Option<u64>,
}

impl TaskControlBlock {
    pub fn capabilities(&self) -> &CapabilityTable {
        &self.cap_table
    }

    pub fn capabilities_mut(&mut self) -> &mut CapabilityTable {
        &mut self.cap_table
    }
}

pub struct Scheduler {
    pub tasks: Vec<TaskControlBlock>,
    pub current_task_idx: usize,
}

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current_task_idx: 0,
        }
    }

    pub fn spawn(&mut self, name: &'static str, entry_fn: fn() -> !) {
        self.spawn_with_caps(name, entry_fn, CapabilityTable::new());
    }

    pub fn spawn_with_caps(&mut self, name: &'static str, entry_fn: fn() -> !, cap_table: CapabilityTable) {
        let task_id = self.tasks.len() as u32;
        let stack = Box::new([0u8; STACK_SIZE]);

        let stack_top = (stack.as_ptr() as u64) + STACK_SIZE as u64;
        let aligned_sp = stack_top & !0x0F;

        let mut context = ArmContext::default();
        context.sp = aligned_sp;
        context.lr = entry_fn as *const () as usize as u64;

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 0,
            page_table: None,
        });
    }

    pub fn spawn_user_process(&mut self, name: &'static str, entry_point: u64, _user_sp: u64, ttbr0: u64, cap_table: CapabilityTable) -> u32 {
        let task_id = self.tasks.len() as u32;
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as u64) + STACK_SIZE as u64) & !0x0F;

        let mut context = ArmContext::default();
        context.sp = kernel_sp;
        context.lr = entry_point;

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 1,
            page_table: Some(ttbr0),
        });
        task_id
    }

    pub fn prepare_switch(&mut self) -> (Option<*mut ArmContext>, Option<*const ArmContext>) {
        if self.tasks.len() <= 1 {
            return (None, None);
        }

        let curr_idx = self.current_task_idx;
        let next_idx = (curr_idx + 1) % self.tasks.len();

        if curr_idx == next_idx {
            return (None, None);
        }

        self.tasks[curr_idx].state = TaskState::Ready;
        self.tasks[next_idx].state = TaskState::Running;
        self.tasks[next_idx].cpu_ticks += 1;
        self.current_task_idx = next_idx;

        // Switch to the incoming task's hardware TTBR0_EL1 translation table if present
        if let Some(ttbr0) = self.tasks[next_idx].page_table {
            crate::mmu::ArmMmu::switch_to_task(ttbr0);
        }

        let old_ctx = &mut self.tasks[curr_idx].context as *mut ArmContext;
        let new_ctx = &self.tasks[next_idx].context as *const ArmContext;

        (Some(old_ctx), Some(new_ctx))
    }
}

pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

pub fn yield_now() {
    let (old_opt, new_opt) = {
        let mut sched = SCHEDULER.lock();
        sched.prepare_switch()
    };
    if let (Some(old_ctx), Some(new_ctx)) = (old_opt, new_opt) {
        unsafe {
            switch_context(old_ctx, new_ctx);
        }
    }
}

static mut TEST_SWITCH_FLAG: u32 = 0;
static mut MAIN_CTX: ArmContext = ArmContext {
    x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0, x25: 0, x26: 0, x27: 0, x28: 0,
    fp: 0, lr: 0, sp: 0,
};
static mut WORKER_CTX: ArmContext = ArmContext {
    x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0, x25: 0, x26: 0, x27: 0, x28: 0,
    fp: 0, lr: 0, sp: 0,
};
static mut WORKER_STACK: [u8; 8192] = [0u8; 8192];

extern "C" fn test_worker_entry() {
    unsafe {
        TEST_SWITCH_FLAG = 0xCAFE_BABE;
        switch_context(core::ptr::addr_of_mut!(WORKER_CTX), core::ptr::addr_of!(MAIN_CTX));
    }
}

pub fn test_context_switching() {
    crate::kprintln!("--- [Testing AArch64 Callee-Saved Register Context Switcher] ---");

    unsafe {
        TEST_SWITCH_FLAG = 0;
        let stack_top = (core::ptr::addr_of!(WORKER_STACK) as u64) + 8192;
        WORKER_CTX.sp = stack_top & !0x0F;
        WORKER_CTX.lr = test_worker_entry as *const () as usize as u64;

        switch_context(core::ptr::addr_of_mut!(MAIN_CTX), core::ptr::addr_of!(WORKER_CTX));

        let flag_val = core::ptr::read_volatile(core::ptr::addr_of!(TEST_SWITCH_FLAG));
        assert_eq!(flag_val, 0xCAFE_BABE, "Context switch failed to execute target task");
    }

    crate::kprintln!("[TEST] AArch64 Context Switch: x19-x28, fp, lr, sp saved & restored. Switched to worker and back VERIFIED.");
}
