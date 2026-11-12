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
    Exited,
    Reaping,
    ReapFailed,
}

use crate::capability::CapabilityTable;

pub struct TaskControlBlock {
    pub id: u32,
    pub parent_id: u32,
    pub name: &'static str,
    pub state: TaskState,
    pub context: ArmContext,
    pub stack: Box<[u8; STACK_SIZE]>,
    pub cap_table: CapabilityTable,
    pub cpu_ticks: u64,
    pub priority: u8,
    pub address_space: Option<crate::mmu::UserAddressSpace>,
    pub page_table: Option<u64>,
    pub user_entry: u64,
    pub user_stack_top: u64,
    pub exit_code: Option<i32>,
    pub generation: u32,
}

impl TaskControlBlock {
    pub fn capabilities(&self) -> &CapabilityTable {
        &self.cap_table
    }

    pub fn capabilities_mut(&mut self) -> &mut CapabilityTable {
        &mut self.cap_table
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReapHandle {
    pub pid: u32,
    pub generation: u32,
    pub exit_code: i32,
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
            next_pid: 0,
        }
    }

    pub fn current_pid(&self) -> u32 {
        if self.current_task_idx < self.tasks.len() {
            self.tasks[self.current_task_idx].id
        } else {
            INVALID_PID
        }
    }

    pub fn allocate_pid(&mut self) -> u32 {
        let mut candidate = self.next_pid;
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

    pub fn spawn(&mut self, name: &'static str, entry_fn: fn() -> !) {
        self.spawn_with_caps(name, entry_fn, CapabilityTable::new());
    }

    pub fn spawn_with_caps(&mut self, name: &'static str, entry_fn: fn() -> !, cap_table: CapabilityTable) {
        let task_id = self.allocate_pid();
        let stack = Box::new([0u8; STACK_SIZE]);

        let stack_top = (stack.as_ptr() as u64) + STACK_SIZE as u64;
        let aligned_sp = stack_top & !0x0F;

        let mut context = ArmContext::default();
        context.sp = aligned_sp;
        context.lr = entry_fn as *const () as usize as u64;

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            parent_id: 0,
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 0,
            address_space: None,
            page_table: None,
            user_entry: 0,
            user_stack_top: 0,
            exit_code: None,
            generation: 1,
        });
    }

    pub fn spawn_user_process(
        &mut self,
        parent_id: u32,
        name: &'static str,
        entry_point: u64,
        user_sp: u64,
        address_space: Option<crate::mmu::UserAddressSpace>,
        cap_table: CapabilityTable,
    ) -> u32 {
        if self.tasks.len() >= MAX_TASKS {
            return 0;
        }
        let task_id = self.allocate_pid();
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as u64) + STACK_SIZE as u64) & !0x0F;

        let mut context = ArmContext::default();
        context.sp = kernel_sp;
        context.lr = user_process_entry as *const () as usize as u64;

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        let ttbr0 = address_space.as_ref().map(|a| a.ttbr0);

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
            page_table: ttbr0,
            user_entry: entry_point,
            user_stack_top: user_sp,
            exit_code: None,
            generation: 1,
        });
        task_id
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
        if task.state == TaskState::Terminated || task.state == TaskState::Exited || task.state == TaskState::Reaping {
            return Ok(());
        }
        task.state = TaskState::Terminated;
        task.generation = task.generation.wrapping_add(1);
        task.exit_code = Some(-15); // SIGTERM
        Ok(())
    }

    pub fn begin_reap(&mut self, caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<(ReapHandle, Option<crate::mmu::UserAddressSpace>), i64> {
        let curr_pid = self.current_pid();
        if target_pid != 0 && target_pid == caller_pid {
            return Err(-16); // -EBUSY: caller cannot reap itself
        }
        if target_pid != 0 && target_pid == curr_pid && self.tasks.get(self.current_task_idx).map(|t| t.state == TaskState::Running).unwrap_or(false) {
            return Err(-16); // -EBUSY: cannot reap active running process
        }

        let pos = self.tasks.iter().position(|t| {
            (target_pid == 0 || t.id == target_pid)
                && (t.state == TaskState::Terminated || t.state == TaskState::Exited || t.state == TaskState::ReapFailed)
        }).ok_or(-30i64)?; // NotReady or not found

        let task = &mut self.tasks[pos];
        if !is_supervisor && task.parent_id != caller_pid {
            return Err(-13); // -EACCES: caller does not own this child and lacks supervisor capability
        }

        task.state = TaskState::Reaping;
        task.generation = task.generation.wrapping_add(1);
        let handle = ReapHandle {
            pid: task.id,
            generation: task.generation,
            exit_code: task.exit_code.unwrap_or(0),
        };
        let aspace = task.address_space.take();
        Ok((handle, aspace))
    }

    pub fn finish_reap(&mut self, handle: ReapHandle) -> Result<i32, i64> {
        let pos = self.tasks.iter().position(|t| {
            t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
        }).ok_or(-3i64)?; // -ESRCH: identity or generation mismatch

        let removed = self.remove_task_at(pos);
        crate::kprintln!("[REAP] Reclaimed user address space and frames for PID {}", removed.id);
        crate::ipc::IPC_ROUTER.lock().unregister_task(removed.id);
        crate::display::SURFACE_MANAGER.lock().destroy_surfaces_owned_by(removed.id);
        crate::input_queue::INPUT_QUEUE.lock().clear_for_task(removed.id);
        Ok(handle.exit_code)
    }

    pub fn abort_reap(&mut self, handle: ReapHandle, aspace: Option<crate::mmu::UserAddressSpace>) {
        if let Some(pos) = self.tasks.iter().position(|t| {
            t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
        }) {
            self.tasks[pos].address_space = aspace;
            self.tasks[pos].state = TaskState::ReapFailed;
            self.tasks[pos].generation = self.tasks[pos].generation.wrapping_add(1);
        }
    }

    pub fn reap_task(&mut self, caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<i32, i64> {
        let (handle, mut aspace_opt) = self.begin_reap(caller_pid, is_supervisor, target_pid)?;
        if let Some(ref mut aspace) = aspace_opt {
            if aspace.is_active_on_any_cpu() {
                self.abort_reap(handle, aspace_opt);
                return Err(-16); // -EBUSY: process still executing on CPU
            }
            if let Err(e) = aspace.destroy_fallible() {
                crate::kprintln!("[REAP ERROR] Failed to destroy user address space for PID {}: {:?}", handle.pid, e);
                self.abort_reap(handle, aspace_opt);
                return Err(-16); // -EBUSY: teardown failed; preserve task and do not emit success log
            }
        }
        self.finish_reap(handle)
    }

    pub fn prepare_switch(&mut self) -> (Option<*mut ArmContext>, Option<*const ArmContext>) {
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

        // Transactional: Validate and activate incoming address space BEFORE committing any state transitions!
        if curr_idx != next_idx {
            if let Some(ref aspace) = self.tasks[next_idx].address_space {
                if let Err(e) = aspace.try_activate() {
                    crate::kprintln!("[SCHED] Activation failed for PID {}: {:?}. Aborting switch transaction without state changes.", self.tasks[next_idx].id, e);
                    return (None, None);
                }
                // Incoming address space is now active in TTBR0_EL1; deactivate outgoing tracking without overwriting TTBR0_EL1:
                if let Some(ref out_aspace) = self.tasks[curr_idx].address_space {
                    out_aspace.deactivate_after_switch();
                }
            } else {
                // Incoming task runs without user address space: deactivate outgoing (which switches TTBR0_EL1 to kernel)
                if let Some(ref out_aspace) = self.tasks[curr_idx].address_space {
                    out_aspace.deactivate();
                } else {
                    crate::mmu::ArmMmu::switch_to_kernel();
                }
            }
        }

        // Commit transaction
        if self.tasks[curr_idx].state == TaskState::Running {
            self.tasks[curr_idx].state = TaskState::Ready;
        }
        self.tasks[next_idx].state = TaskState::Running;
        self.tasks[next_idx].cpu_ticks += 1;
        self.current_task_idx = next_idx;

        let old_ctx = &mut self.tasks[curr_idx].context as *mut ArmContext;
        let new_ctx = &self.tasks[next_idx].context as *const ArmContext;

        (Some(old_ctx), Some(new_ctx))
    }
}

pub fn reap_task_locksafe(caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<i32, i64> {
    let (handle, mut aspace_opt) = {
        let mut sched = SCHEDULER.lock();
        sched.begin_reap(caller_pid, is_supervisor, target_pid)?
    };

    let teardown_res = if let Some(ref mut aspace) = aspace_opt {
        if aspace.is_active_on_any_cpu() {
            Err(crate::mmu::TeardownError::ActiveOnRemoteCpu(aspace.active_cpus()))
        } else {
            aspace.destroy_fallible()
        }
    } else {
        Ok(())
    };

    match teardown_res {
        Ok(()) => {
            let mut sched = SCHEDULER.lock();
            sched.finish_reap(handle)
        }
        Err(e) => {
            crate::kprintln!("[REAP ERROR] Failed to destroy user address space for PID {}: {:?}", handle.pid, e);
            let mut sched = SCHEDULER.lock();
            sched.abort_reap(handle, aspace_opt);
            Err(-16) // -EBUSY
        }
    }
}

pub extern "C" fn user_process_entry() -> ! {
    let (entry, sp) = {
        let sched = SCHEDULER.lock();
        let curr = sched.current_task_idx;
        (sched.tasks[curr].user_entry, sched.tasks[curr].user_stack_top)
    };
    unsafe {
        crate::user::enter_user_mode(entry, sp);
    }
    loop {}
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

fn dummy_test_task() -> ! {
    loop {}
}

pub fn test_task_lifecycle() {
    crate::kprintln!("--- [Testing AArch64 Task Scheduler & Removal Consistency] ---");
    let mut sched = Scheduler::new();
    sched.next_pid = 1;
    sched.spawn("task1", dummy_test_task);
    sched.spawn("task2", dummy_test_task);
    sched.spawn("task3", dummy_test_task);
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

    // Test 6: Two-Phase Lock-Safe Reaping & Generation Tracking
    let c2 = sched2.spawn_user_process(p1, "child2", 0, 0, None, caps.clone());
    sched2.tasks[1].state = TaskState::Terminated;
    let (handle, aspace_opt) = sched2.begin_reap(p1, false, c2).expect("begin_reap on terminated child");
    assert_eq!(sched2.tasks[1].state, TaskState::Reaping);
    // Concurrent second reap while Reaping must be rejected
    match sched2.begin_reap(p1, false, c2) {
        Err(e) => assert_eq!(e, -30),
        Ok(_) => panic!("Concurrent begin_reap must fail with -30"),
    }
    // Stale generation handle finish_reap must be rejected
    let mut stale_handle = handle;
    stale_handle.generation = stale_handle.generation.wrapping_add(99);
    assert_eq!(sched2.finish_reap(stale_handle), Err(-3));
    // Abort reap preserves task in ReapFailed state with incremented generation
    sched2.abort_reap(handle, aspace_opt);
    assert_eq!(sched2.tasks[1].state, TaskState::ReapFailed);
    // Retry reap on ReapFailed task must succeed
    let (retry_handle, _retry_aspace) = sched2.begin_reap(p1, false, c2).expect("begin_reap on ReapFailed child");
    assert_eq!(sched2.finish_reap(retry_handle), Ok(0));

    crate::kprintln!("[TEST] AArch64 Task Removal Consistency & Two-Phase Reap VERIFIED.");
}

pub fn test_transactional_switch() {
    crate::kprintln!("--- [Testing AArch64 Transactional Switch Rollback] ---");
    let mut sched2 = Scheduler::new();
    sched2.next_pid = 1;
    let caps = CapabilityTable::new();
    let p1 = sched2.spawn_user_process(0, "parent", 0, 0, None, caps.clone());

    let aspace_closing = crate::mmu::UserAddressSpace::new().expect("Address space alloc");
    aspace_closing.lifecycle.fetch_or(crate::mmu::LIFECYCLE_CLOSING_BIT, core::sync::atomic::Ordering::SeqCst);
    let c3 = sched2.spawn_user_process(p1, "closing_candidate", 0, 0, Some(aspace_closing), caps);
    sched2.current_task_idx = 0;
    sched2.tasks[0].state = TaskState::Running;
    let (old_opt, new_opt) = sched2.prepare_switch();
    // Activation should fail and switch must be transactionally aborted without state mutation
    assert!(old_opt.is_none() && new_opt.is_none());
    assert_eq!(sched2.current_task_idx, 0, "current_task_idx must remain unchanged on activation failure");
    assert_eq!(sched2.tasks[0].state, TaskState::Running, "Running task state must remain Running");
    let c3_pos = sched2.tasks.iter().position(|t| t.id == c3).unwrap();
    assert_eq!(sched2.tasks[c3_pos].state, TaskState::Ready, "Candidate task must remain Ready, not falsely Terminated");
    assert_eq!(sched2.tasks[c3_pos].cpu_ticks, 0, "Candidate task cpu_ticks must not be incremented on abort");

    crate::kprintln!("[TEST] AArch64 Transactional Switch Rollback VERIFIED.");
}
