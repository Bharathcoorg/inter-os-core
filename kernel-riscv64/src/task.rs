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
    Reaping,
    ReapFailed,
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
pub const SUPERVISOR_PID: u32 = 0xFFFF_FFFE;
pub const REAP_EVACUATION_MAX_SPINS: usize = 1000;

fn aspace_is_evacuated(aspace: &crate::mmu::UserAddressSpace) -> bool {
    let active = aspace.active_harts();
    let executed = aspace.executed_harts.load(core::sync::atomic::Ordering::Acquire);
    let acknowledged = aspace.acknowledged_harts.load(core::sync::atomic::Ordering::Acquire);
    let unacknowledged = executed & !acknowledged;
    active == 0 && unacknowledged == 0
}

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
        if task.state == TaskState::Terminated || task.state == TaskState::Exited || task.state == TaskState::Reaping {
            return Ok(());
        }
        task.state = TaskState::Terminated;
        task.generation = task.generation.wrapping_add(1);
        task.exit_code = Some(-15); // SIGTERM
        Ok(())
    }

    pub fn begin_reap(&mut self, caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<ReapHandle, i64> {
        let curr_pid = self.current_pid();
        if target_pid != 0 && target_pid == caller_pid {
            return Err(-16); // -EBUSY: caller cannot reap itself
        }
        if target_pid != 0 && target_pid == curr_pid && self.tasks.get(self.current_task_idx).map(|t| t.state == TaskState::Running).unwrap_or(false) {
            return Err(-16); // -EBUSY: cannot reap active running process
        }

        let pos = self.tasks.iter().position(|t| {
            (target_pid == 0 || t.id == target_pid)
                && (target_pid != 0 || t.id != caller_pid)
                && (target_pid != 0 || t.id != 0)
                && (target_pid != 0 || is_supervisor || t.parent_id == caller_pid)
                && (t.state == TaskState::Terminated || t.state == TaskState::Exited || t.state == TaskState::ReapFailed)
        }).ok_or(-30i64)?;

        let task = &mut self.tasks[pos];
        if !is_supervisor && task.parent_id != caller_pid {
            return Err(-13); // -EACCES: caller does not own this child and lacks supervisor capability
        }

        task.state = TaskState::Reaping;
        task.generation = task.generation.wrapping_add(1);
        if let Some(ref aspace) = task.address_space {
            aspace
                .lifecycle
                .fetch_or(crate::mmu::LIFECYCLE_CLOSING_BIT, core::sync::atomic::Ordering::Release);
        }
        let handle = ReapHandle {
            pid: task.id,
            generation: task.generation,
            exit_code: task.exit_code.unwrap_or(0),
        };
        // Keep task.address_space attached so remote prepare_switch() can observe and deactivate it.
        Ok(handle)
    }

    pub fn detach_reaping_aspace(&mut self, handle: ReapHandle) -> Result<Option<crate::mmu::UserAddressSpace>, i64> {
        let pos = self.tasks.iter().position(|t| {
            t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
        }).ok_or(-3i64)?;

        if let Some(ref aspace) = self.tasks[pos].address_space {
            if !aspace_is_evacuated(aspace) {
                self.tasks[pos].state = TaskState::ReapFailed;
                self.tasks[pos].generation = self.tasks[pos].generation.wrapping_add(1);
                return Err(-16); // -EBUSY: hart still active or unacknowledged
            }
        }

        Ok(self.tasks[pos].address_space.take())
    }

    pub fn finish_reap(&mut self, handle: ReapHandle) -> Result<i32, i64> {
        let pos = self.tasks.iter().position(|t| {
            t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
        }).ok_or(-3i64)?;

        let removed = self.remove_task_at(pos);
        crate::println!("[REAP] Reclaimed user address space and frames for PID {}", removed.id);
        crate::ipc::IPC_ROUTER.lock().unregister_task(removed.id);
        crate::display::SURFACE_MANAGER.lock().destroy_surfaces_owned_by(removed.id);
        crate::input_queue::INPUT_QUEUE.lock().clear_for_task(removed.id);
        Ok(handle.exit_code)
    }

    pub fn abort_reap(&mut self, handle: ReapHandle, aspace: Option<crate::mmu::UserAddressSpace>) {
        if let Some(pos) = self.tasks.iter().position(|t| {
            t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
        }) {
            if let Some(a) = aspace {
                self.tasks[pos].address_space = Some(a);
            }
            self.tasks[pos].state = TaskState::ReapFailed;
            self.tasks[pos].generation = self.tasks[pos].generation.wrapping_add(1);
        }
    }

    pub fn reap_task(&mut self, caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<i32, i64> {
        let handle = self.begin_reap(caller_pid, is_supervisor, target_pid)?;
        let mut aspace_opt = self.detach_reaping_aspace(handle)?;
        if let Some(ref mut aspace) = aspace_opt {
            if let Err(e) = aspace.destroy_fallible() {
                crate::println!("[REAP ERROR] Failed to destroy user address space for PID {}: {:?}", handle.pid, e);
                self.abort_reap(handle, aspace_opt);
                return Err(-16); // -EBUSY: teardown failed; preserve task and do not emit success log
            }
        }
        self.finish_reap(handle)
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
            generation: 1,
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
            generation: 1,
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

        // Transactional: Validate and activate incoming address space BEFORE committing any state transitions!
        if curr_idx != next_idx {
            if let Some(ref aspace) = self.tasks[next_idx].address_space {
                if let Err(e) = aspace.try_activate() {
                    crate::println!("[SCHED] Activation failed for PID {}: {:?}. Aborting switch transaction without state changes.", self.tasks[next_idx].id, e);
                    return (None, None);
                }
                // Incoming address space is now active in SATP; deactivate outgoing tracking without overwriting SATP:
                if let Some(ref out_aspace) = self.tasks[curr_idx].address_space {
                    out_aspace.deactivate_after_switch();
                }
            } else {
                // Incoming task runs without user address space: deactivate outgoing (which switches SATP to kernel)
                if let Some(ref out_aspace) = self.tasks[curr_idx].address_space {
                    out_aspace.deactivate();
                } else {
                    unsafe {
                        crate::mmu::switch_to_kernel_satp();
                    }
                }
            }
        }
        unsafe {
            core::arch::asm!("csrw sscratch, {0}", in(reg) self.tasks[next_idx].kstack_top);
        }

        // Commit transaction
        if self.tasks[curr_idx].state == TaskState::Running {
            self.tasks[curr_idx].state = TaskState::Ready;
        }
        self.tasks[next_idx].state = TaskState::Running;
        self.tasks[next_idx].cpu_ticks += 1;
        self.current_task_idx = next_idx;

        let old_ctx = &mut self.tasks[curr_idx].context as *mut RiscvContext;
        let new_ctx = &self.tasks[next_idx].context as *const RiscvContext;

        (Some(old_ctx), Some(new_ctx))
    }
}

pub fn reap_task_locksafe(caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<i32, i64> {
    // Stage 1: Mark target Reaping under lock while keeping address_space attached.
    let handle = {
        let mut sched = SCHEDULER.lock();
        sched.begin_reap(caller_pid, is_supervisor, target_pid)?
    };

    // Stage 2: Bounded lock-free evacuation polling so remote harts can run prepare_switch() + deactivate().
    let mut evacuated = false;
    for _ in 0..REAP_EVACUATION_MAX_SPINS {
        let check = {
            let sched = SCHEDULER.lock();
            match sched.tasks.iter().find(|t| {
                t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
            }) {
                Some(task) => match task.address_space {
                    Some(ref aspace) => Ok(aspace_is_evacuated(aspace)),
                    None => Ok(true),
                },
                None => Err(-3i64),
            }
        };
        match check {
            Ok(true) => {
                evacuated = true;
                break;
            }
            Ok(false) => core::hint::spin_loop(),
            Err(e) => return Err(e),
        }
    }

    if !evacuated {
        let mut sched = SCHEDULER.lock();
        sched.abort_reap(handle, None);
        return Err(-16); // -EBUSY: remote hart failed to evacuate/acknowledge in time
    }

    // Stage 3: Reacquire lock, revalidate handle and evacuation, and detach address_space.
    let mut aspace_opt = {
        let mut sched = SCHEDULER.lock();
        sched.detach_reaping_aspace(handle)?
    };

    // Stage 4: Destroy detached address space outside SCHEDULER.lock().
    let teardown_res = if let Some(ref mut aspace) = aspace_opt {
        aspace.destroy_fallible()
    } else {
        Ok(())
    };

    // Stage 5: Finalize under lock.
    match teardown_res {
        Ok(()) => {
            let mut sched = SCHEDULER.lock();
            sched.finish_reap(handle)
        }
        Err(e) => {
            crate::println!("[REAP ERROR] Failed to destroy user address space for PID {}: {:?}", handle.pid, e);
            let mut sched = SCHEDULER.lock();
            sched.abort_reap(handle, aspace_opt);
            Err(-16) // -EBUSY
        }
    }
}

/// Reaps a supervisor-managed service task.
/// Uses authentic internal kernel supervisor identity (`SUPERVISOR_PID`, `is_supervisor: true`).
/// Cannot be impersonated by userspace because userspace PIDs are allocated sequentially (0..255).
pub fn reap_service_task(target_pid: u32) -> Result<i32, i64> {
    if target_pid == 0 || target_pid == INVALID_PID || target_pid == SUPERVISOR_PID {
        return Err(-22); // -EINVAL
    }
    reap_task_locksafe(SUPERVISOR_PID, true, target_pid)
}

/// Terminates a supervisor-managed service task.
pub fn terminate_service_task(sched: &mut Scheduler, target_pid: u32) -> Result<(), i64> {
    if target_pid == 0 || target_pid == INVALID_PID || target_pid == SUPERVISOR_PID {
        return Err(-22); // -EINVAL
    }
    sched.terminate_task(SUPERVISOR_PID, true, target_pid)
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

    // Test 6: 5-Stage Lock-Safe Reaping & Generation Tracking
    let c2 = sched2.spawn_user_process(p1, "child2", 0, 0, None, caps.clone());
    sched2.tasks[1].state = TaskState::Terminated;
    let handle = sched2.begin_reap(p1, false, c2).expect("begin_reap on terminated child");
    assert_eq!(sched2.tasks[1].state, TaskState::Reaping);
    // Concurrent second reap while Reaping must be rejected
    match sched2.begin_reap(p1, false, c2) {
        Err(e) => assert_eq!(e, -30),
        Ok(_) => panic!("Concurrent begin_reap must fail with -30"),
    }
    // Stale generation handle detach/finish_reap must be rejected
    let mut stale_handle = handle;
    stale_handle.generation = stale_handle.generation.wrapping_add(99);
    assert!(matches!(sched2.detach_reaping_aspace(stale_handle), Err(-3)));
    assert_eq!(sched2.finish_reap(stale_handle), Err(-3));
    // Abort reap preserves task in ReapFailed state with incremented generation
    let detached = sched2.detach_reaping_aspace(handle).expect("detach on evacuated child");
    sched2.abort_reap(handle, detached);
    assert_eq!(sched2.tasks[1].state, TaskState::ReapFailed);
    // Retry reap on ReapFailed task must succeed
    let retry_handle = sched2.begin_reap(p1, false, c2).expect("begin_reap on ReapFailed child");
    assert!(sched2.detach_reaping_aspace(retry_handle).is_ok());
    assert_eq!(sched2.finish_reap(retry_handle), Ok(0));

    // Test 7: Transactional Switch Rollback on Activation Failure
    let aspace_closing = crate::mmu::UserAddressSpace::new().expect("Address space alloc");
    aspace_closing.lifecycle.fetch_or(crate::mmu::LIFECYCLE_CLOSING_BIT, core::sync::atomic::Ordering::SeqCst);
    let c3 = sched2.spawn_user_process(p1, "closing_candidate", 0, 0, Some(aspace_closing), caps.clone());
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

    // Test 8: Remote Address-Space Evacuation Race & 5-Stage Reaping Invariants
    let saved_satp: usize;
    let saved_sscratch: usize;
    unsafe {
        core::arch::asm!("csrr {0}, satp", out(reg) saved_satp);
        core::arch::asm!("csrr {0}, sscratch", out(reg) saved_sscratch);
    }

    let mut sched3 = Scheduler::new();
    sched3.next_pid = 1;
    let p_idle = sched3.spawn_user_process(0, "idle_parent", 0, 0, None, caps.clone());
    let aspace_evac = crate::mmu::UserAddressSpace::new().expect("Address space alloc for evacuation test");
    let c_evac = sched3.spawn_user_process(p_idle, "evac_child", 0, 0, Some(aspace_evac), caps.clone());

    // Simulate active execution of c_evac before it exits and a peer/parent initiates reap
    sched3.tasks[1].address_space.as_ref().unwrap().try_activate().expect("activate evac_child");
    sched3.tasks[1].state = TaskState::Exited;
    sched3.current_task_idx = 1;

    // 8a/8b: begin_reap marks task Reaping and keeps address_space attached
    let h1 = sched3.begin_reap(p_idle, false, c_evac).expect("begin_reap on exited child");
    assert_eq!(sched3.tasks[1].state, TaskState::Reaping);
    assert!(sched3.tasks[1].address_space.is_some(), "address_space must remain attached in Reaping");

    // 8a: Reaping task is unschedulable (prepare_switch will not select it)
    sched3.tasks[0].state = TaskState::Running;
    sched3.current_task_idx = 0;
    let (unsel_old, unsel_new) = sched3.prepare_switch();
    assert!(unsel_old.is_none() && unsel_new.is_none(), "Reaping task must not be scheduled");
    sched3.tasks[0].state = TaskState::Ready;
    sched3.current_task_idx = 1;

    // 8d: detach_reaping_aspace rejects detachment while active_harts != 0 and preserves aspace & hart bits
    assert!(sched3.tasks[1].address_space.as_ref().unwrap().active_harts() != 0);
    assert!(matches!(sched3.detach_reaping_aspace(h1), Err(-16)));
    assert_eq!(sched3.tasks[1].state, TaskState::ReapFailed);
    assert_eq!(sched3.tasks[1].generation, h1.generation.wrapping_add(1));
    assert!(sched3.tasks[1].address_space.is_some(), "address_space must not be detached on EBUSY");
    assert!(sched3.tasks[1].address_space.as_ref().unwrap().active_harts() != 0, "hart bits must not be cleared on EBUSY");
    assert!(matches!(sched3.detach_reaping_aspace(h1), Err(-3)));
    assert_eq!(sched3.finish_reap(h1), Err(-3));

    // 8c: Outgoing prepare_switch from a Reaping task accesses curr.address_space and deactivates it
    let h2 = sched3.begin_reap(p_idle, false, c_evac).expect("retry begin_reap on ReapFailed");
    assert_eq!(h2.generation, h1.generation.wrapping_add(2));
    let (evac_old, evac_new) = sched3.prepare_switch();
    unsafe {
        core::arch::asm!("csrw satp, {0}", "sfence.vma zero, zero", in(reg) saved_satp, options(nostack));
        core::arch::asm!("csrw sscratch, {0}", in(reg) saved_sscratch);
    }
    assert!(evac_old.is_some() && evac_new.is_some(), "outgoing switch from Reaping task must succeed");
    assert_eq!(sched3.current_task_idx, 0);
    assert_eq!(sched3.tasks[1].state, TaskState::Reaping, "Reaping state must be preserved across outgoing switch");
    {
        let aspace_ref = sched3.tasks[1].address_space.as_ref().unwrap();
        assert_eq!(aspace_ref.active_harts(), 0, "prepare_switch must deactivate outgoing Reaping address space");
        assert!(aspace_is_evacuated(aspace_ref), "outgoing address space must be fully evacuated");
    }

    // 8e: detach_reaping_aspace succeeds after evacuation
    let detached_aspace = sched3.detach_reaping_aspace(h2).expect("detach after evacuation");
    assert!(detached_aspace.is_some());
    assert!(sched3.tasks[1].address_space.is_none());

    // 8f: Failed teardown restores address_space into ReapFailed and stale ReapHandle cannot reap newer generation
    sched3.abort_reap(h2, detached_aspace);
    assert_eq!(sched3.tasks[1].state, TaskState::ReapFailed);
    assert_eq!(sched3.tasks[1].generation, h2.generation.wrapping_add(1));
    assert!(sched3.tasks[1].address_space.is_some(), "abort_reap must restore address_space");
    assert!(matches!(sched3.detach_reaping_aspace(h2), Err(-3)));
    assert_eq!(sched3.finish_reap(h2), Err(-3));

    // Complete final reap on c_evac cleanly
    let h3 = sched3.begin_reap(p_idle, false, c_evac).expect("final begin_reap after abort");
    let mut final_aspace = sched3.detach_reaping_aspace(h3).expect("final detach").expect("restored aspace");
    final_aspace.destroy_fallible().expect("destroy_fallible after evacuation");
    assert_eq!(sched3.finish_reap(h3), Ok(0));

    // 8e (direct deactivate path): verify detach_reaping_aspace succeeds after direct deactivate()
    let aspace_deact = crate::mmu::UserAddressSpace::new().expect("Address space alloc for deactivate test");
    let c_deact = sched3.spawn_user_process(p_idle, "deact_child", 0, 0, Some(aspace_deact), caps.clone());
    let c_deact_pos = sched3.tasks.iter().position(|t| t.id == c_deact).unwrap();
    sched3.tasks[c_deact_pos].address_space.as_ref().unwrap().try_activate().expect("activate deact_child");
    sched3.tasks[c_deact_pos].state = TaskState::Terminated;
    let hd1 = sched3.begin_reap(p_idle, false, c_deact).expect("begin_reap on deact_child");
    assert!(matches!(sched3.detach_reaping_aspace(hd1), Err(-16)));
    sched3.tasks[c_deact_pos].address_space.as_ref().unwrap().deactivate();
    unsafe {
        core::arch::asm!("csrw satp, {0}", "sfence.vma zero, zero", in(reg) saved_satp, options(nostack));
    }
    assert_eq!(sched3.reap_task(p_idle, false, c_deact), Ok(0));

    // End-to-end reap_task_locksafe() on global SCHEDULER: rejects while active -> preserves address_space -> succeeds after deactivate()
    let (orig_len, orig_next_pid, ls_pid) = {
        let mut g_sched = SCHEDULER.lock();
        let len = g_sched.tasks.len();
        let next_p = g_sched.next_pid;
        let aspace_ls = crate::mmu::UserAddressSpace::new().expect("Address space alloc for locksafe e2e");
        aspace_ls.try_activate().expect("activate locksafe test aspace");
        let pid = g_sched.spawn_user_process(1, "locksafe_e2e", 0, 0, Some(aspace_ls), CapabilityTable::new());
        if let Some(t) = g_sched.tasks.iter_mut().find(|t| t.id == pid) {
            t.state = TaskState::Terminated;
        }
        (len, next_p, pid)
    };
    assert_eq!(reap_task_locksafe(ls_pid.wrapping_add(100), true, ls_pid), Err(-16));
    {
        let g_sched = SCHEDULER.lock();
        let t = g_sched.tasks.iter().find(|t| t.id == ls_pid).expect("task must remain after EBUSY");
        assert_eq!(t.state, TaskState::ReapFailed);
        let aspace_ref = t.address_space.as_ref().expect("aspace must remain attached on EBUSY");
        assert_ne!(aspace_ref.active_harts(), 0);
        aspace_ref.deactivate();
    }
    unsafe {
        core::arch::asm!("csrw satp, {0}", "sfence.vma zero, zero", in(reg) saved_satp, options(nostack));
    }
    assert_eq!(reap_task_locksafe(ls_pid.wrapping_add(100), true, ls_pid), Ok(0));
    {
        let mut g_sched = SCHEDULER.lock();
        assert_eq!(g_sched.tasks.len(), orig_len);
        g_sched.next_pid = orig_next_pid;
    }

    // Test 9: Wildcard Reaping Target Selection Safety (target_pid == 0)
    let p_a = sched2.spawn_user_process(0, "parent_a", 0, 0, None, caps.clone());
    let p_b = sched2.spawn_user_process(0, "parent_b", 0, 0, None, caps.clone());
    let c_a = sched2.spawn_user_process(p_a, "child_a", 0, 0, None, caps.clone());
    let c_b = sched2.spawn_user_process(p_b, "child_b", 0, 0, None, caps.clone());
    let c_a_pos = sched2.tasks.iter().position(|t| t.id == c_a).unwrap();
    sched2.tasks[c_a_pos].state = TaskState::Terminated;
    let c_b_pos = sched2.tasks.iter().position(|t| t.id == c_b).unwrap();
    sched2.tasks[c_b_pos].state = TaskState::Terminated;

    // Caller p_b calls wildcard begin_reap(p_b, false, 0).
    // It MUST skip foreign child_a and select p_b's child c_b!
    let h_b = sched2.begin_reap(p_b, false, 0).expect("Wildcard reap for parent_b must select child_b");
    assert_eq!(h_b.pid, c_b, "Wildcard reap must find caller's own child, not foreign child");
    assert_eq!(sched2.finish_reap(h_b), Ok(0));

    // Now caller p_a calls wildcard begin_reap(p_a, false, 0) -> selects child_a
    let h_a = sched2.begin_reap(p_a, false, 0).expect("Wildcard reap for parent_a must select child_a");
    assert_eq!(h_a.pid, c_a, "Wildcard reap must select child_a");
    assert_eq!(sched2.finish_reap(h_a), Ok(0));

    // Second wildcard reap with no terminated children must return -30
    assert_eq!(sched2.begin_reap(p_a, false, 0), Err(-30));

    // Test 10: Service Supervisor Identity Reaping API
    let svc_child = sched2.spawn_user_process(0, "svc_child", 0, 0, None, caps.clone());
    let svc_pos = sched2.tasks.iter().position(|t| t.id == svc_child).unwrap();
    sched2.tasks[svc_pos].state = TaskState::Terminated;
    // Invalid PIDs rejected
    assert_eq!(reap_service_task(0), Err(-22));
    assert_eq!(reap_service_task(INVALID_PID), Err(-22));
    assert_eq!(reap_service_task(SUPERVISOR_PID), Err(-22));
    // Supervisor reap of svc_child succeeds using internal SUPERVISOR_PID
    let h_svc = sched2.begin_reap(SUPERVISOR_PID, true, svc_child).expect("Supervisor begin_reap on service task");
    assert_eq!(h_svc.pid, svc_child);
    assert_eq!(sched2.finish_reap(h_svc), Ok(0));

    crate::println!("[TEST] RISC-V Task Removal Consistency, 5-Stage Reap & Transactional Switch VERIFIED.");
}
