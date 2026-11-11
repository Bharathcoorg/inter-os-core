// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Task Scheduler & Callee-Saved Register Context Switching
//!
//! Implements cooperative and preemptive multitasking across RISC-V 64 CPU cores.
//! Context switches between task execution stacks using standard RISC-V ABI conventions:
//! - Callee-saved general-purpose registers: s0-s11 (x8-x9, x18-x27)
//! - Return address (ra / x1)
//! - Stack pointer (sp / x2)
//! - Per-task Sv39 address space (SATP)

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;

pub const STACK_SIZE: usize = 16384; // 16 KiB per kernel task stack

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskId(pub usize);

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct RiscvContext {
    pub ra: u64,
    pub sp: u64,
    pub s0: u64,
    pub s1: u64,
    pub s2: u64,
    pub s3: u64,
    pub s4: u64,
    pub s5: u64,
    pub s6: u64,
    pub s7: u64,
    pub s8: u64,
    pub s9: u64,
    pub s10: u64,
    pub s11: u64,
}

#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(old_ctx: *mut RiscvContext, new_ctx: *const RiscvContext) {
    core::arch::naked_asm!(
        "sd ra, 0*8(a0)",
        "sd sp, 1*8(a0)",
        "sd s0, 2*8(a0)",
        "sd s1, 3*8(a0)",
        "sd s2, 4*8(a0)",
        "sd s3, 5*8(a0)",
        "sd s4, 6*8(a0)",
        "sd s5, 7*8(a0)",
        "sd s6, 8*8(a0)",
        "sd s7, 9*8(a0)",
        "sd s8, 10*8(a0)",
        "sd s9, 11*8(a0)",
        "sd s10, 12*8(a0)",
        "sd s11, 13*8(a0)",

        "ld ra, 0*8(a1)",
        "ld sp, 1*8(a1)",
        "ld s0, 2*8(a1)",
        "ld s1, 3*8(a1)",
        "ld s2, 4*8(a1)",
        "ld s3, 5*8(a1)",
        "ld s4, 6*8(a1)",
        "ld s5, 7*8(a1)",
        "ld s6, 8*8(a1)",
        "ld s7, 9*8(a1)",
        "ld s8, 10*8(a1)",
        "ld s9, 11*8(a1)",
        "ld s10, 12*8(a1)",
        "ld s11, 13*8(a1)",

        "ret",
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Terminated,
    Exited,
}

use crate::capability::CapabilityTable;

pub struct TaskControlBlock {
    pub id: u32,
    pub name: &'static str,
    pub state: TaskState,
    pub context: RiscvContext,
    pub stack: Box<[u8; STACK_SIZE]>,
    pub cap_table: CapabilityTable,
    pub cpu_ticks: u64,
    pub priority: u8,
    pub page_table: Option<usize>, // SATP
    pub user_entry: usize,
    pub user_stack_top: usize,
    pub kstack_top: usize,
    pub exit_code: Option<i32>,
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

    pub fn current_task_id(&self) -> TaskId {
        if self.current_task_idx < self.tasks.len() {
            TaskId(self.tasks[self.current_task_idx].id as usize)
        } else {
            TaskId(1)
        }
    }

    pub fn exit_current(&mut self) -> bool {
        if self.current_task_idx < self.tasks.len() {
            let task = &mut self.tasks[self.current_task_idx];
            task.state = TaskState::Exited;
            true
        } else {
            false
        }
    }

    pub fn add_task(
        &mut self,
        _id: TaskId,
        satp: usize,
        entry_point: usize,
        user_sp: usize,
        kstack_top: usize,
    ) -> bool {
        let task_id = self.tasks.len() as u32;
        let stack = Box::new([0u8; STACK_SIZE]);
        let mut context = RiscvContext::default();
        context.sp = kstack_top as u64;
        context.ra = user_process_entry as *const () as usize as u64;

        let mut cap_table = CapabilityTable::new();
        let _ = cap_table.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
        let _ = cap_table.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            name: "primordial_init",
            state: TaskState::Running,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 1,
            page_table: Some(satp),
            user_entry: entry_point,
            user_stack_top: user_sp,
            kstack_top,
            exit_code: None,
        });
        true
    }

    pub fn spawn_user_process(
        &mut self,
        name: &'static str,
        entry_point: usize,
        user_sp: usize,
        satp: usize,
        cap_table: CapabilityTable,
    ) -> u32 {
        let task_id = self.tasks.len() as u32;
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as usize) + STACK_SIZE) & !0x0F;

        let mut context = RiscvContext::default();
        context.sp = kernel_sp as u64;
        context.ra = user_process_entry as *const () as usize as u64;

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
            page_table: Some(satp),
            user_entry: entry_point,
            user_stack_top: user_sp,
            kstack_top: kernel_sp,
            exit_code: None,
        });
        task_id
    }

    pub fn prepare_switch(&mut self) -> (Option<*mut RiscvContext>, Option<*const RiscvContext>) {
        if self.tasks.len() <= 1 {
            return (None, None);
        }

        let curr_idx = self.current_task_idx;
        let mut next_idx = (curr_idx + 1) % self.tasks.len();
        let mut count = 0;
        while self.tasks[next_idx].state != TaskState::Ready && count < self.tasks.len() {
            next_idx = (next_idx + 1) % self.tasks.len();
            count += 1;
        }

        if curr_idx == next_idx || self.tasks[next_idx].state != TaskState::Ready {
            return (None, None);
        }

        if self.tasks[curr_idx].state == TaskState::Running {
            self.tasks[curr_idx].state = TaskState::Ready;
        }
        self.tasks[next_idx].state = TaskState::Running;
        self.tasks[next_idx].cpu_ticks += 1;
        self.current_task_idx = next_idx;

        if let Some(satp) = self.tasks[next_idx].page_table {
            unsafe {
                crate::mmu::switch_satp(satp);
                core::arch::asm!("csrw sscratch, {0}", in(reg) self.tasks[next_idx].kstack_top);
            }
        }

        let old_ctx = &mut self.tasks[curr_idx].context as *mut RiscvContext;
        let new_ctx = &self.tasks[next_idx].context as *const RiscvContext;

        (Some(old_ctx), Some(new_ctx))
    }
}

pub extern "C" fn user_process_entry() -> ! {
    let (entry, sp, kstack) = {
        let sched = SCHEDULER.lock();
        let curr = sched.current_task_idx;
        (
            sched.tasks[curr].user_entry,
            sched.tasks[curr].user_stack_top,
            sched.tasks[curr].kstack_top,
        )
    };
    unsafe {
        crate::trap::enter_user_mode(entry, sp, kstack);
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
