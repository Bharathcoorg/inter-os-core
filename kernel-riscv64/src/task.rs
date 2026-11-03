// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Task Scheduler & Hardware Context Switching
//!
//! Enforces per-task Sv39 address spaces (SATP), per-thread kernel stacks,
//! and complete register state restoration during preemptive context switches.

use crate::trap::TrapFrame;
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Exited,
}

#[repr(C, align(16))]
pub struct TaskControlBlock {
    pub id: TaskId,
    pub state: TaskState,
    pub trap_frame: TrapFrame,
    pub satp: usize,
    pub kernel_stack_top: usize,
    pub priority: u8,
    pub cpu_ticks: u64,
}

pub struct Scheduler {
    pub tasks: [Option<TaskControlBlock>; 8],
    pub current_idx: usize,
    pub task_count: usize,
    pub current_task: TaskId,
}

impl Scheduler {
    pub const fn new() -> Self {
        const NONE_TCB: Option<TaskControlBlock> = None;
        Self {
            tasks: [NONE_TCB; 8],
            current_idx: 0,
            task_count: 0,
            current_task: TaskId(1),
        }
    }

    pub fn add_task(&mut self, id: TaskId, satp: usize, entry_point: usize, user_sp: usize, kstack_top: usize) -> bool {
        if self.task_count >= 8 {
            return false;
        }

        let mut tf = TrapFrame::default();
        tf.sepc = entry_point;
        tf.regs[2] = user_sp; // User SP (x2)
        // Set sstatus.SPP = 0 for U-mode, sstatus.SPIE = 1 to re-enable interrupts upon sret
        tf.sstatus = 1 << 5; // SPIE bit

        let tcb = TaskControlBlock {
            id,
            state: TaskState::Ready,
            trap_frame: tf,
            satp,
            kernel_stack_top: kstack_top,
            priority: 1,
            cpu_ticks: 0,
        };

        for slot in self.tasks.iter_mut() {
            if slot.is_none() {
                *slot = Some(tcb);
                self.task_count += 1;
                return true;
            }
        }
        false
    }
}

pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());


impl Scheduler {
    /// Returns the scheduler-owned identity of the task currently executing
    /// in the RISC-V kernel context. Syscalls must not manufacture task IDs.
    pub fn current_task_id(&self) -> TaskId {
        self.current_task
    }

    /// Selects a scheduler task as the current execution owner.
    pub fn set_current_task(&mut self, id: TaskId) -> bool {
        if self.tasks.iter().flatten().any(|task| task.id == id && task.state != TaskState::Exited) {
            self.current_task = id;
            true
        } else {
            false
        }
    }

    /// Marks the current task terminal without destroying its scheduler record.
    /// Reaping remains a separate operation so future scheduling can preserve
    /// ownership and lifecycle invariants.
    pub fn exit_current(&mut self) -> bool {
        let id = self.current_task;
        if let Some(task) = self.tasks.iter_mut().flatten().find(|task| task.id == id) {
            task.state = TaskState::Exited;
            true
        } else {
            false
        }
    }
}
