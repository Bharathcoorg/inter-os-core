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
    pub parent_id: u32,
    pub name: &'static str,
    pub state: TaskState,
    pub context: RiscvContext,
    pub stack: Box<[u8; STACK_SIZE]>,
    pub cap_table: CapabilityTable,
    pub cpu_ticks: u64,
    pub priority: u8,
    pub address_space: Option<crate::mmu::UserAddressSpace>,
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
    pub next_pid: u32,
}

pub const MAX_TASKS: usize = 256;
pub const INVALID_PID: u32 = u32::MAX;

impl Scheduler {
    pub const fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current_task_idx: 0,
            next_pid: 1,
        }
    }

    pub fn current_pid(&self) -> u32 {
        if self.current_task_idx < self.tasks.len() {
            self.tasks[self.current_task_idx].id
        } else {
            INVALID_PID
        }
    }

    pub fn current_task_id(&self) -> TaskId {
        TaskId(self.current_pid() as usize)
    }

    pub fn allocate_pid(&mut self) -> u32 {
        let mut candidate = self.next_pid.max(1);
        let mut attempts = 0;
        while self.tasks.iter().any(|t| t.id == candidate) {
            candidate = candidate.wrapping_add(1);
            if candidate == 0 || candidate == INVALID_PID {
                candidate = 1;
            }
            attempts += 1;
            if attempts > 65536 {
                break;
            }
        }
        self.next_pid = candidate.wrapping_add(1);
        if self.next_pid == 0 || self.next_pid == INVALID_PID {
            self.next_pid = 1;
        }
        candidate
    }

    pub fn remove_task_at(&mut self, pos: usize) -> TaskControlBlock {
        let removed = self.tasks.remove(pos);
        if pos < self.current_task_idx {
            self.current_task_idx = self.current_task_idx.saturating_sub(1);
        } else if self.current_task_idx >= self.tasks.len() && !self.tasks.is_empty() {
            self.current_task_idx = self.tasks.len() - 1;
        }
        removed
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

    pub fn terminate_task(&mut self, caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<(), i64> {
        if target_pid == 0 {
            return Err(-22); // -EINVAL
        }
        let pos = self.tasks.iter().position(|t| t.id == target_pid).ok_or(-3)?; // -ESRCH
        let task = &mut self.tasks[pos];
        if !is_supervisor && task.parent_id != caller_pid && task.id != caller_pid {
            return Err(-13); // -EACCES
        }
        if task.state == TaskState::Terminated || task.state == TaskState::Exited {
            return Ok(());
        }
        task.state = TaskState::Terminated;
        task.exit_code = Some(-15); // SIGTERM
        Ok(())
    }

    pub fn reap_task(&mut self, caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<i32, i64> {
        let curr_pid = self.current_pid();
        if target_pid != 0 && target_pid == caller_pid {
            return Err(-16); // -EBUSY: caller cannot reap itself
        }
        if target_pid != 0 && target_pid == curr_pid && self.tasks.get(self.current_task_idx).map(|t| t.state == TaskState::Running).unwrap_or(false) {
            return Err(-16); // -EBUSY: cannot reap active running process
        }

        let pos = self.tasks.iter().position(|t| {
            (target_pid == 0 || t.id == target_pid)
                && (t.state == TaskState::Terminated || t.state == TaskState::Exited)
        }).ok_or(-30)?;

        let task = &self.tasks[pos];
        if !is_supervisor && task.parent_id != caller_pid {
            return Err(-13); // -EACCES: caller does not own this child and lacks supervisor capability
        }

        if let Some(ref mut aspace) = self.tasks[pos].address_space {
            if aspace.is_active_on_any_hart() {
                return Err(-16); // -EBUSY: process still executing on hart
            }
            if let Err(e) = aspace.destroy_fallible() {
                crate::println!("[REAP ERROR] Failed to destroy user address space for PID {}: {:?}", self.tasks[pos].id, e);
                return Err(-16); // -EBUSY: teardown failed; preserve task and do not emit success log
            }
        }

        let removed = self.remove_task_at(pos);
        crate::println!("[REAP] Reclaimed user address space and frames for PID {}", removed.id);
        crate::ipc::IPC_ROUTER.lock().unregister_task(removed.id);
        crate::display::SURFACE_MANAGER.lock().destroy_surfaces_owned_by(removed.id);
        crate::input_queue::INPUT_QUEUE.lock().clear_for_task(removed.id);
        Ok(removed.exit_code.unwrap_or(0))
    }

    pub fn add_task(
        &mut self,
        _id: TaskId,
        satp: usize,
        entry_point: usize,
        user_sp: usize,
        kstack_top: usize,
    ) -> bool {
        if self.tasks.len() >= MAX_TASKS {
            return false;
        }
        let task_id = if _id.0 != 0 { _id.0 as u32 } else { self.allocate_pid() };
        if self.tasks.iter().any(|t| t.id == task_id) {
            return false; // Prevent duplicate task registration
        }
        if self.next_pid <= task_id {
            self.next_pid = task_id.wrapping_add(1);
            if self.next_pid == 0 || self.next_pid == INVALID_PID {
                self.next_pid = 1;
            }
        }
        let stack = Box::new([0u8; STACK_SIZE]);
        let mut context = RiscvContext::default();
        context.sp = kstack_top as u64;
        context.ra = user_process_entry as *const () as usize as u64;

        let mut cap_table = CapabilityTable::new();
        let _ = cap_table.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
        let _ = cap_table.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);
        let _ = cap_table.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE | crate::capability::Rights::ADMIN, None);
        let _ = cap_table.mint(crate::capability::CapabilityType::SystemResource, crate::capability::Rights::READ | crate::capability::Rights::WRITE | crate::capability::Rights::ADMIN, None);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            parent_id: 0,
            name: "primordial_init",
            state: TaskState::Running,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 1,
            address_space: None,
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
        parent_id: u32,
        name: &'static str,
        entry_point: usize,
        user_sp: usize,
        address_space: Option<crate::mmu::UserAddressSpace>,
        cap_table: CapabilityTable,
    ) -> u32 {
        if self.tasks.len() >= MAX_TASKS {
            return 0;
        }
        let task_id = self.allocate_pid();
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as usize) + STACK_SIZE) & !0x0F;

        let mut context = RiscvContext::default();
        context.sp = kernel_sp as u64;
        context.ra = user_process_entry as *const () as usize as u64;

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        let satp = address_space.as_ref().map(|a| a.satp);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            parent_id,
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 1,
            address_space,
            page_table: satp,
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

        if curr_idx != next_idx {
            // Deactivate outgoing task address space on current hart
            if let Some(ref aspace) = self.tasks[curr_idx].address_space {
                aspace.deactivate();
            }
            // Activate incoming task address space on current hart
            if let Some(ref aspace) = self.tasks[next_idx].address_space {
                if let Err(e) = aspace.try_activate() {
                    crate::println!("[SCHED] Activation failed for PID {}: {:?}. Aborting switch.", self.tasks[next_idx].id, e);
                    self.tasks[next_idx].state = TaskState::Terminated;
                    self.tasks[next_idx].exit_code = Some(-1);
                    unsafe {
                        crate::mmu::switch_to_kernel_satp();
                    }
                    return (None, None);
                }
            } else {
                unsafe {
                    crate::mmu::switch_to_kernel_satp();
                }
            }
        }
        unsafe {
            core::arch::asm!("csrw sscratch, {0}", in(reg) self.tasks[next_idx].kstack_top);
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

pub fn test_task_lifecycle() {
    crate::println!("--- [Testing RISC-V Task Scheduler & Removal Consistency] ---");
    let mut sched = Scheduler::new();
    sched.next_pid = 1;
    sched.add_task(TaskId(1), 0, 0, 0, 0);
    let caps = CapabilityTable::new();
    sched.spawn_user_process(1, "task2", 0, 0, None, caps.clone());
    sched.spawn_user_process(1, "task3", 0, 0, None, caps.clone());
    assert_eq!(sched.tasks.len(), 3);
    assert_eq!(sched.tasks[0].id, 1);
    assert_eq!(sched.tasks[1].id, 2);
    assert_eq!(sched.tasks[2].id, 3);

    // Test 1: removing middle task when current_task_idx is 2 (task3)
    sched.current_task_idx = 2;
    let removed_mid = sched.remove_task_at(1);
    assert_eq!(removed_mid.id, 2);
    assert_eq!(sched.current_task_idx, 1);
    assert_eq!(sched.tasks[sched.current_task_idx].id, 3);

    // Test 2: removing first task when current_task_idx is 1 (task3)
    let removed_first = sched.remove_task_at(0);
    assert_eq!(removed_first.id, 1);
    assert_eq!(sched.current_task_idx, 0);
    assert_eq!(sched.tasks[sched.current_task_idx].id, 3);

    // Test 3: removing final task
    let removed_final = sched.remove_task_at(0);
    assert_eq!(removed_final.id, 3);
    assert_eq!(sched.tasks.len(), 0);
    assert_eq!(sched.current_task_idx, 0);

    // Test 4: Reaping authority and active process protection
    let mut sched2 = Scheduler::new();
    sched2.next_pid = 1;
    let caps = CapabilityTable::new();
    let p1 = sched2.spawn_user_process(0, "parent", 0, 0, None, caps.clone());
    let c1 = sched2.spawn_user_process(p1, "child", 0, 0, None, caps.clone());
    sched2.current_task_idx = 0; // Current running is parent (p1)

    // Active task cannot be reaped
    assert_eq!(sched2.reap_task(p1, true, p1), Err(-16));

    // Child is still Ready, not Terminated/Exited
    assert_eq!(sched2.reap_task(p1, false, c1), Err(-30));

    // Mark child terminated
    sched2.tasks[1].state = TaskState::Terminated;

    // Non-parent without supervisor authority is denied
    assert_eq!(sched2.reap_task(999, false, c1), Err(-13));

    // Parent can reap child
    assert_eq!(sched2.reap_task(p1, false, c1), Ok(0));

    // Idempotent rejection on second reap
    assert_eq!(sched2.reap_task(p1, false, c1), Err(-30));

    // Test 5: PID monotonicity and collision avoidance
    sched.next_pid = u32::MAX - 1;
    let pid_a = sched.allocate_pid();
    assert_eq!(pid_a, u32::MAX - 1);
    let pid_b = sched.allocate_pid();
    assert_eq!(pid_b, 1); // Skips u32::MAX (INVALID_PID) and 0

    crate::println!("[TEST] RISC-V Task Removal Consistency & Reap Security VERIFIED.");
}
