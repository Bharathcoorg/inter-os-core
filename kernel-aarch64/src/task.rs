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
#[repr(C, align(16))]
pub struct ArmContext {
    /// 31 general-purpose registers: x0-x30 (x29=FP, x30=LR) = 31 * 8 = 248 bytes
    pub x: [u64; 31],
    /// Dedicated stack pointer SP_EL0 = 8 bytes (offset 248)
    pub sp: u64,
    /// Exception Link Register ELR_EL1 / Return PC = 8 bytes (offset 256)
    pub elr: u64,
    /// Saved Program Status Register SPSR_EL1 = 8 bytes (offset 264)
    pub spsr: u64,
    /// Full 128-bit SIMD / FP vector register bank: q0-q31 = 32 * 16 = 512 bytes (offset 272)
    pub q: [u128; 32],
    /// Floating-Point Control Register = 8 bytes (offset 784)
    pub fpcr: u64,
    /// Floating-Point Status Register = 8 bytes (offset 792)
    pub fpsr: u64,
}

// Compile-time verification of layout, total size (800 bytes), 16-byte alignment, and exact field offsets
const _: () = {
    assert!(core::mem::size_of::<ArmContext>() == 800);
    assert!(core::mem::align_of::<ArmContext>() == 16);
    assert!(core::mem::offset_of!(ArmContext, x) == 0);
    assert!(core::mem::offset_of!(ArmContext, sp) == 248);
    assert!(core::mem::offset_of!(ArmContext, elr) == 256);
    assert!(core::mem::offset_of!(ArmContext, spsr) == 264);
    assert!(core::mem::offset_of!(ArmContext, q) == 272);
    assert!(core::mem::offset_of!(ArmContext, fpcr) == 784);
    assert!(core::mem::offset_of!(ArmContext, fpsr) == 792);
};

impl ArmContext {
    pub const fn empty() -> Self {
        Self {
            x: [0u64; 31],
            sp: 0,
            elr: 0,
            spsr: 0,
            q: [0u128; 32],
            fpcr: 0,
            fpsr: 0,
        }
    }

    #[inline]
    pub fn lr(&self) -> u64 {
        self.x[30]
    }

    #[inline]
    pub fn set_lr(&mut self, val: u64) {
        self.x[30] = val;
        self.elr = val;
    }
}

/// Enables standard NEON/FP floating-point execution in EL0 and EL1 while strictly
/// trapping unsupported SVE (ZEN=0) and SME (SMEN=0) instructions to prevent state leakage.
pub fn init_fp_simd() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let mut cpacr: u64;
        core::arch::asm!("mrs {}, cpacr_el1", out(reg) cpacr, options(nomem, nostack, preserves_flags));
        // Set FPEN bits [21:20] to 0b11 (enable full FP/SIMD at EL0 and EL1)
        cpacr |= 3u64 << 20;
        // Clear ZEN bits [17:16] to 0b00 (trap SVE instructions to #UD/exception)
        cpacr &= !(3u64 << 16);
        // Clear SMEN bits [25:24] to 0b00 (trap SME instructions)
        cpacr &= !(3u64 << 24);
        core::arch::asm!("msr cpacr_el1, {}", in(reg) cpacr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(old_ctx: *mut ArmContext, new_ctx: *const ArmContext) {
    core::arch::naked_asm!(
        // Save callee-saved registers x19-x30 into old_ctx.x (x0)
        "stp x19, x20, [x0, #152]",
        "stp x21, x22, [x0, #168]",
        "stp x23, x24, [x0, #184]",
        "stp x25, x26, [x0, #200]",
        "stp x27, x28, [x0, #216]",
        "stp x29, x30, [x0, #232]",
        // Save current stack pointer (offset 248) and return address (offset 256)
        "mov x2, sp",
        "str x2, [x0, #248]",
        "str x30, [x0, #256]",
        // Save full 128-bit vector register bank q0-q31 (512 bytes at offset 272)
        "stp q0, q1, [x0, #272]",
        "stp q2, q3, [x0, #304]",
        "stp q4, q5, [x0, #336]",
        "stp q6, q7, [x0, #368]",
        "stp q8, q9, [x0, #400]",
        "stp q10, q11, [x0, #432]",
        "stp q12, q13, [x0, #464]",
        "stp q14, q15, [x0, #496]",
        "stp q16, q17, [x0, #528]",
        "stp q18, q19, [x0, #560]",
        "stp q20, q21, [x0, #592]",
        "stp q22, q23, [x0, #624]",
        "stp q24, q25, [x0, #656]",
        "stp q26, q27, [x0, #688]",
        "stp q28, q29, [x0, #720]",
        "stp q30, q31, [x0, #752]",
        // Save FPCR and FPSR floating point status/control (offset 784)
        "mrs x2, fpcr",
        "mrs x3, fpsr",
        "stp x2, x3, [x0, #784]",

        // Restore callee-saved registers x19-x30 from new_ctx.x (x1)
        "ldp x19, x20, [x1, #152]",
        "ldp x21, x22, [x1, #168]",
        "ldp x23, x24, [x1, #184]",
        "ldp x25, x26, [x1, #200]",
        "ldp x27, x28, [x1, #216]",
        "ldp x29, x30, [x1, #232]",
        // Restore stack pointer
        "ldr x2, [x1, #248]",
        "mov sp, x2",
        // Restore full 128-bit vector register bank q0-q31
        "ldp q0, q1, [x1, #272]",
        "ldp q2, q3, [x1, #304]",
        "ldp q4, q5, [x1, #336]",
        "ldp q6, q7, [x1, #368]",
        "ldp q8, q9, [x1, #400]",
        "ldp q10, q11, [x1, #432]",
        "ldp q12, q13, [x1, #464]",
        "ldp q14, q15, [x1, #496]",
        "ldp q16, q17, [x1, #528]",
        "ldp q18, q19, [x1, #560]",
        "ldp q20, q21, [x1, #592]",
        "ldp q22, q23, [x1, #624]",
        "ldp q24, q25, [x1, #656]",
        "ldp q26, q27, [x1, #688]",
        "ldp q28, q29, [x1, #720]",
        "ldp q30, q31, [x1, #752]",
        // Restore FPCR and FPSR
        "ldp x2, x3, [x1, #784]",
        "msr fpcr, x2",
        "msr fpsr, x3",

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
    pub is_service: bool,
    pub service_id: Option<u8>,
    pub service_generation: Option<u32>,
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

pub fn is_reserved_pid(pid: u32) -> bool {
    pid == 0 || pid == SUPERVISOR_PID || pid == INVALID_PID
}

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

    pub fn allocate_pid(&mut self) -> Option<u32> {
        let start = if is_reserved_pid(self.next_pid) { 1 } else { self.next_pid };
        let mut candidate = start;
        loop {
            if !is_reserved_pid(candidate) && !self.tasks.iter().any(|t| t.id == candidate) {
                let mut next = candidate.wrapping_add(1);
                if is_reserved_pid(next) {
                    next = 1;
                }
                self.next_pid = next;
                return Some(candidate);
            }
            candidate = candidate.wrapping_add(1);
            if is_reserved_pid(candidate) {
                candidate = 1;
            }
            if candidate == start {
                return None;
            }
        }
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
        let task_id = self.allocate_pid().expect("PID allocation failure during kernel daemon spawn");
        let stack = Box::new([0u8; STACK_SIZE]);

        let stack_top = (stack.as_ptr() as u64) + STACK_SIZE as u64;
        let aligned_sp = stack_top & !0x0F;

        let mut context = ArmContext::default();
        context.sp = aligned_sp;
        context.set_lr(entry_fn as *const () as usize as u64);

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
            is_service: false,
            service_id: None,
            service_generation: None,
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
        let task_id = match self.allocate_pid() {
            Some(id) => id,
            None => return 0,
        };
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as u64) + STACK_SIZE as u64) & !0x0F;

        let mut context = ArmContext::default();
        context.sp = kernel_sp;
        context.set_lr(user_process_entry as *const () as usize as u64);

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
            is_service: false,
            service_id: None,
            service_generation: None,
        });
        task_id
    }

    pub fn add_task(&mut self, id: u32, entry_point: u64, user_sp: u64) -> bool {
        if is_reserved_pid(id) {
            return false;
        }
        if self.tasks.iter().any(|t| t.id == id) {
            return false; // Prevent duplicate task registration
        }
        if self.tasks.len() >= MAX_TASKS {
            return false;
        }
        if self.next_pid <= id {
            self.next_pid = id.wrapping_add(1);
            if is_reserved_pid(self.next_pid) {
                self.next_pid = 1;
            }
        }
        let stack = Box::new([0u8; STACK_SIZE]);
        let kernel_sp = ((stack.as_ptr() as u64) + STACK_SIZE as u64) & !0x0F;
        let mut context = ArmContext::default();
        context.sp = kernel_sp;
        context.set_lr(user_process_entry as *const () as usize as u64);

        let cap_table = CapabilityTable::new();
        self.tasks.push(TaskControlBlock {
            id,
            parent_id: 0,
            name: "primordial_task",
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority: 1,
            address_space: None,
            page_table: None,
            user_entry: entry_point,
            user_stack_top: user_sp,
            exit_code: None,
            generation: 1,
            is_service: false,
            service_id: None,
            service_generation: None,
        });
        true
    }

    pub(crate) fn spawn_service_process(
        &mut self,
        service_id: u8,
        generation: u32,
        name: &'static str,
        entry_point: u64,
        user_sp: u64,
        address_space: Option<crate::mmu::UserAddressSpace>,
        cap_table: CapabilityTable,
    ) -> u32 {
        if generation == 0 || !crate::services::is_valid_service_id(service_id) {
            return 0;
        }
        if self.tasks.len() >= MAX_TASKS {
            return 0;
        }
        let task_id = match self.allocate_pid() {
            Some(id) => id,
            None => return 0,
        };
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as u64) + STACK_SIZE as u64) & !0x0F;

        let mut context = ArmContext::default();
        context.sp = kernel_sp;
        context.set_lr(user_process_entry as *const () as usize as u64);

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        let ttbr0 = address_space.as_ref().map(|a| a.ttbr0);

        self.tasks.push(TaskControlBlock {
            id: task_id,
            parent_id: SUPERVISOR_PID,
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
            is_service: true,
            service_id: Some(service_id),
            service_generation: Some(generation),
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

    /// Returns true only when no CPU is actively using `aspace` and every CPU
    /// that executed in `aspace` has acknowledged evacuation.
    pub fn is_aspace_evacuated(aspace: &crate::mmu::UserAddressSpace) -> bool {
        if aspace.is_active_on_any_cpu() {
            return false;
        }
        let executed = aspace
            .executed_cpus
            .load(core::sync::atomic::Ordering::Acquire);
        let acknowledged = aspace
            .acknowledged_cpus
            .load(core::sync::atomic::Ordering::Acquire);
        (executed & !acknowledged) == 0
    }

    /// Stage 1 of the 5-stage reaping protocol (must be called with `SCHEDULER.lock()` held):
    /// transitions the task to `TaskState::Reaping`, increments `generation`, and returns a
    /// `ReapHandle` while keeping `task.address_space` attached so remote `prepare_switch()`
    /// calls can still observe and deactivate it.
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
        }).ok_or(-30i64)?; // NotReady or not found

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
        Ok(ReapHandle {
            pid: task.id,
            generation: task.generation,
            exit_code: task.exit_code.unwrap_or(0),
        })
    }

    /// Stage 3 of the 5-stage reaping protocol (must be called with `SCHEDULER.lock()` held):
    /// re-validates task identity, generation, `TaskState::Reaping`, and full CPU evacuation
    /// under lock before detaching `address_space`. If still active/unacknowledged, marks the
    /// task `TaskState::ReapFailed`, bumps `generation`, and returns `Err(-16)` without detaching.
    pub fn detach_reaping_aspace(
        &mut self,
        handle: ReapHandle,
    ) -> Result<Option<crate::mmu::UserAddressSpace>, i64> {
        let pos = self.tasks.iter().position(|t| {
            t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
        }).ok_or(-3i64)?; // -ESRCH: identity or generation mismatch

        if let Some(ref aspace) = self.tasks[pos].address_space {
            if !Self::is_aspace_evacuated(aspace) {
                self.abort_reap(handle, None);
                return Err(-16); // -EBUSY: address space still active or unacknowledged on a CPU
            }
        }

        Ok(self.tasks[pos].address_space.take())
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
            // Only overwrite address_space when restoring a previously detached one;
            // pre-detach aborts pass `None` and must keep the attached address space intact.
            if let Some(restored) = aspace {
                self.tasks[pos].address_space = Some(restored);
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
    // Stage 1: Mark target Reaping under lock while keeping address_space attached.
    let handle = {
        let mut sched = SCHEDULER.lock();
        sched.begin_reap(caller_pid, is_supervisor, target_pid)?
    };

    // Stage 2: Bounded evacuation check outside SCHEDULER.lock() so remote CPUs
    // can acquire the lock, execute prepare_switch(), and deactivate the outgoing address space.
    const MAX_EVACUATION_POLLS: usize = 1000;
    let mut evacuated = false;
    for _ in 0..MAX_EVACUATION_POLLS {
        let check = {
            let sched = SCHEDULER.lock();
            match sched.tasks.iter().position(|t| {
                t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
            }) {
                Some(pos) => match sched.tasks[pos].address_space.as_ref() {
                    Some(aspace) => Ok(Scheduler::is_aspace_evacuated(aspace)),
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
        return Err(-16); // -EBUSY: CPU evacuation did not complete within bounded wait
    }

    // Stage 3: Reacquire lock, verify evacuation under lock, and detach address_space.
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

    // Stage 5: Finalize or roll back under lock using the original ReapHandle.
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

/// Reaps a supervisor-managed service task.
/// Validates caller's `ServiceLifecycleToken` against the live service registry and scheduler TCB.
/// Rejects non-service tasks, stale tokens, and wrong service IDs with -EPERM (-1).
pub(crate) fn reap_service_task(token: &crate::services::ServiceLifecycleToken) -> Result<i32, i64> {
    if !token.verify_live() {
        return Err(-1); // -EPERM
    }
    let target_pid = token.pid();
    if is_reserved_pid(target_pid) {
        return Err(-22); // -EINVAL
    }
    {
        let sched = SCHEDULER.lock();
        let t = match sched.tasks.iter().find(|t| t.id == target_pid) {
            Some(t) => t,
            None => return Err(-30), // -ESRCH
        };
        if !t.is_service || t.service_id != Some(token.service_id()) || t.service_generation != Some(token.generation()) || t.parent_id != SUPERVISOR_PID {
            return Err(-1); // -EPERM
        }
    }
    reap_task_locksafe(SUPERVISOR_PID, true, target_pid)
}

/// Terminates a supervisor-managed service task.
/// Validates caller's `ServiceLifecycleToken` against the live service registry and scheduler TCB.
/// Rejects non-service tasks, stale tokens, and wrong service IDs with -EPERM (-1).
pub(crate) fn terminate_service_task(
    sched: &mut Scheduler,
    token: &crate::services::ServiceLifecycleToken,
) -> Result<(), i64> {
    if !token.verify_live() {
        return Err(-1); // -EPERM
    }
    let target_pid = token.pid();
    if is_reserved_pid(target_pid) {
        return Err(-22); // -EINVAL
    }
    let t = match sched.tasks.iter().find(|t| t.id == target_pid) {
        Some(t) => t,
        None => return Err(-3), // -ESRCH
    };
    if !t.is_service || t.service_id != Some(token.service_id()) || t.service_generation != Some(token.generation()) || t.parent_id != SUPERVISOR_PID {
        return Err(-1); // -EPERM
    }
    sched.terminate_task(SUPERVISOR_PID, true, target_pid)
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
static mut MAIN_CTX: ArmContext = ArmContext::empty();
static mut WORKER_CTX: ArmContext = ArmContext::empty();
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
        let mut wctx = WORKER_CTX;
        wctx.set_lr(test_worker_entry as *const () as usize as u64);
        WORKER_CTX = wctx;

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

    // Test 5: PID monotonicity, collision avoidance, and reserved PID skip
    sched.next_pid = SUPERVISOR_PID - 1;
    let pid_a = sched.allocate_pid().expect("allocate before supervisor pid");
    assert_eq!(pid_a, SUPERVISOR_PID - 1);
    let pid_b = sched.allocate_pid().expect("allocate wraps to 1");
    assert_eq!(pid_b, 1); // Skips SUPERVISOR_PID (u32::MAX - 1), INVALID_PID (u32::MAX), and 0
    assert!(is_reserved_pid(0));
    assert!(is_reserved_pid(SUPERVISOR_PID));
    assert!(is_reserved_pid(INVALID_PID));
    assert!(!is_reserved_pid(1));
    assert!(!is_reserved_pid(SUPERVISOR_PID - 1));

    // Test 6: Multi-Stage Lock-Safe Reaping & Generation Tracking
    let c2 = sched2.spawn_user_process(p1, "child2", 0, 0, None, caps.clone());
    sched2.tasks[1].state = TaskState::Terminated;
    let handle = sched2.begin_reap(p1, false, c2).expect("begin_reap on terminated child");
    assert_eq!(sched2.tasks[1].state, TaskState::Reaping);
    // Reaping task must be unschedulable (prepare_switch ignores Reaping)
    sched2.current_task_idx = 0;
    sched2.tasks[0].state = TaskState::Running;
    let (old_opt, new_opt) = sched2.prepare_switch();
    assert!(old_opt.is_none() && new_opt.is_none(), "prepare_switch must ignore Reaping task");
    // Concurrent second reap while Reaping must be rejected
    match sched2.begin_reap(p1, false, c2) {
        Err(e) => assert_eq!(e, -30),
        Ok(_) => panic!("Concurrent begin_reap must fail with -30"),
    }
    // Stale generation handle finish_reap & detach_reaping_aspace must be rejected
    let mut stale_handle = handle;
    stale_handle.generation = stale_handle.generation.wrapping_add(99);
    assert_eq!(sched2.finish_reap(stale_handle), Err(-3));
    assert!(matches!(sched2.detach_reaping_aspace(stale_handle), Err(-3)));
    // Abort reap preserves task in ReapFailed state with incremented generation
    sched2.abort_reap(handle, None);
    assert_eq!(sched2.tasks[1].state, TaskState::ReapFailed);
    assert_eq!(sched2.tasks[1].generation, handle.generation.wrapping_add(1));
    assert_eq!(sched2.finish_reap(handle), Err(-3), "Old handle must not reap newer generation");
    // Retry reap on ReapFailed task must succeed
    let retry_handle = sched2.begin_reap(p1, false, c2).expect("begin_reap on ReapFailed child");
    let retry_aspace = sched2.detach_reaping_aspace(retry_handle).expect("detach on ReapFailed retry");
    assert!(retry_aspace.is_none());
    assert_eq!(sched2.finish_reap(retry_handle), Ok(0));

    // Test 7: Wildcard Reaping Target Selection Safety (target_pid == 0)
    let p_a = sched2.spawn_user_process(0, "parent_a", 0, 0, None, caps.clone());
    let p_b = sched2.spawn_user_process(0, "parent_b", 0, 0, None, caps.clone());
    let c_a = sched2.spawn_user_process(p_a, "child_a", 0, 0, None, caps.clone());
    let c_b = sched2.spawn_user_process(p_b, "child_b", 0, 0, None, caps.clone());
    // Terminate child_a first so it is earlier in the task array than child_b
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

    // Test 8: Service Supervisor Identity Reaping API
    let svc_child = sched2.spawn_service_process(3, 1, "svc_child", 0, 0, None, caps.clone());
    let svc_pos = sched2.tasks.iter().position(|t| t.id == svc_child).unwrap();
    sched2.tasks[svc_pos].state = TaskState::Terminated;
    // Supervisor reap of svc_child succeeds using internal SUPERVISOR_PID
    let h_svc = sched2.begin_reap(SUPERVISOR_PID, true, svc_child).expect("Supervisor begin_reap on service task");
    assert_eq!(h_svc.pid, svc_child);
    assert_eq!(sched2.finish_reap(h_svc), Ok(0));

    crate::kprintln!("[TEST] AArch64 Task Removal Consistency & Multi-Stage Reap VERIFIED.");
}

pub fn test_reap_evacuation_protocol() {
    crate::kprintln!("--- [Testing AArch64 5-Stage Reap Evacuation Protocol] ---");
    let mut sched = Scheduler::new();
    sched.next_pid = 5000;
    let caps = CapabilityTable::new();
    let p1 = sched.spawn_user_process(0, "reap_parent", 0, 0, None, caps.clone());

    // 1. Reaping marks task unschedulable (prepare_switch skips Reaping and picks Ready)
    let c_reap = sched.spawn_user_process(p1, "reaping_child", 0, 0, None, caps.clone());
    let c_ready = sched.spawn_user_process(p1, "ready_child", 0, 0, None, caps.clone());
    let c_reap_pos = sched.tasks.iter().position(|t| t.id == c_reap).unwrap();
    sched.tasks[c_reap_pos].state = TaskState::Terminated;
    let h_skip = sched.begin_reap(p1, false, c_reap).expect("begin_reap on c_reap");
    sched.current_task_idx = 0;
    sched.tasks[0].state = TaskState::Running;
    let (old_opt, new_opt) = sched.prepare_switch();
    assert!(old_opt.is_some() && new_opt.is_some());
    assert_eq!(sched.tasks[sched.current_task_idx].id, c_ready, "prepare_switch must skip Reaping task");
    let c_reap_pos = sched.tasks.iter().position(|t| t.id == c_reap).unwrap();
    assert_eq!(sched.tasks[c_reap_pos].state, TaskState::Reaping);
    let _ = sched.detach_reaping_aspace(h_skip).expect("detach none aspace");
    assert_eq!(sched.finish_reap(h_skip), Ok(0));

    // Reset p1 as Ready for subsequent switch tests
    let p1_pos = sched.tasks.iter().position(|t| t.id == p1).unwrap();
    sched.tasks[p1_pos].state = TaskState::Ready;
    // Remove c_ready so only p1 and the tested child exist
    let c_ready_pos = sched.tasks.iter().position(|t| t.id == c_ready).unwrap();
    sched.tasks[c_ready_pos].state = TaskState::Terminated;
    assert_eq!(sched.reap_task(p1, false, c_ready), Ok(0));

    // 2 & 3. Address space remains attached in TaskState::Reaping after begin_reap,
    // and outgoing prepare_switch from a Reaping task still accesses and deactivates curr.address_space.
    let aspace_out = crate::mmu::UserAddressSpace::new().expect("Address space alloc");
    let c_out = sched.spawn_user_process(p1, "outgoing_reap", 0, 0, Some(aspace_out), caps.clone());
    let c_out_pos = sched.tasks.iter().position(|t| t.id == c_out).unwrap();
    sched.tasks[c_out_pos]
        .address_space
        .as_ref()
        .unwrap()
        .try_activate()
        .expect("activate outgoing aspace");
    assert!(sched.tasks[c_out_pos].address_space.as_ref().unwrap().is_active_on_any_cpu());
    sched.tasks[c_out_pos].state = TaskState::Terminated;
    let h_out = sched.begin_reap(p1, false, c_out).expect("begin_reap on outgoing task");
    assert_eq!(sched.tasks[c_out_pos].state, TaskState::Reaping);
    assert!(sched.tasks[c_out_pos].address_space.is_some(), "address_space must stay attached in Reaping");

    // Simulate remote/outgoing CPU switching away from the Reaping task
    sched.current_task_idx = c_out_pos;
    let (old_opt, new_opt) = sched.prepare_switch();
    assert!(old_opt.is_some() && new_opt.is_some());
    assert_eq!(sched.tasks[sched.current_task_idx].id, p1);
    let c_out_pos = sched.tasks.iter().position(|t| t.id == c_out).unwrap();
    assert_eq!(sched.tasks[c_out_pos].state, TaskState::Reaping, "Outgoing Reaping task must stay Reaping");
    {
        let aspace_ref = sched.tasks[c_out_pos].address_space.as_ref().expect("aspace still attached");
        assert!(!aspace_ref.is_active_on_any_cpu(), "prepare_switch must deactivate outgoing Reaping aspace");
        assert!(Scheduler::is_aspace_evacuated(aspace_ref), "outgoing Reaping aspace must be acknowledged");
    }
    let mut detached_out = sched
        .detach_reaping_aspace(h_out)
        .expect("detach after outgoing prepare_switch")
        .expect("expected Some(aspace)");
    assert!(detached_out.destroy_fallible().is_ok());
    assert_eq!(sched.finish_reap(h_out), Ok(0));

    // 4, 5 & 6. detach_reaping_aspace rejects while active_cpus != 0, succeeds once deactivate() called,
    // abort_reap restores address_space & sets ReapFailed, and stale ReapHandle cannot reap newer generation.
    let aspace_guard = crate::mmu::UserAddressSpace::new().expect("Address space alloc");
    let c_guard = sched.spawn_user_process(p1, "guarded_reap", 0, 0, Some(aspace_guard), caps);
    let c_guard_pos = sched.tasks.iter().position(|t| t.id == c_guard).unwrap();
    sched.tasks[c_guard_pos]
        .address_space
        .as_ref()
        .unwrap()
        .try_activate()
        .expect("activate guarded aspace");
    assert_ne!(sched.tasks[c_guard_pos].address_space.as_ref().unwrap().active_cpus(), 0);
    sched.tasks[c_guard_pos].state = TaskState::Terminated;
    let h_active = sched.begin_reap(p1, false, c_guard).expect("begin_reap while active");

    // Must reject detachment while active_cpus != 0 and preserve attached address_space
    assert!(matches!(sched.detach_reaping_aspace(h_active), Err(-16)));
    let c_guard_pos = sched.tasks.iter().position(|t| t.id == c_guard).unwrap();
    assert_eq!(sched.tasks[c_guard_pos].state, TaskState::ReapFailed);
    assert_eq!(sched.tasks[c_guard_pos].generation, h_active.generation.wrapping_add(1));
    assert!(sched.tasks[c_guard_pos].address_space.is_some(), "failed detach must not drop address_space");

    // Evacuate CPU via deactivate(), then detach must succeed
    sched.tasks[c_guard_pos].address_space.as_ref().unwrap().deactivate();
    assert_eq!(sched.tasks[c_guard_pos].address_space.as_ref().unwrap().active_cpus(), 0);
    let h_retry = sched.begin_reap(p1, false, c_guard).expect("retry begin_reap after deactivate");
    let detached = sched
        .detach_reaping_aspace(h_retry)
        .expect("detach after deactivate")
        .expect("expected Some(aspace)");
    let c_guard_pos = sched.tasks.iter().position(|t| t.id == c_guard).unwrap();
    assert!(sched.tasks[c_guard_pos].address_space.is_none());
    assert_eq!(sched.tasks[c_guard_pos].state, TaskState::Reaping);
    assert_eq!(sched.tasks[c_guard_pos].generation, h_retry.generation);

    // Failed teardown via abort_reap restores address_space, sets ReapFailed, and invalidates old ReapHandle
    sched.abort_reap(h_retry, Some(detached));
    let c_guard_pos = sched.tasks.iter().position(|t| t.id == c_guard).unwrap();
    assert!(sched.tasks[c_guard_pos].address_space.is_some(), "abort_reap must restore address_space");
    assert_eq!(sched.tasks[c_guard_pos].state, TaskState::ReapFailed);
    assert_eq!(sched.tasks[c_guard_pos].generation, h_retry.generation.wrapping_add(1));
    assert_eq!(sched.finish_reap(h_retry), Err(-3), "stale ReapHandle must not reap newer generation");
    assert!(matches!(sched.detach_reaping_aspace(h_retry), Err(-3)));

    // Clean final reap so zero frames or IPC mailboxes leak
    let h_final = sched.begin_reap(p1, false, c_guard).expect("final begin_reap");
    let mut final_aspace = sched
        .detach_reaping_aspace(h_final)
        .expect("final detach")
        .expect("expected Some(aspace)");
    assert!(final_aspace.destroy_fallible().is_ok());
    assert_eq!(sched.finish_reap(h_final), Ok(0));

    crate::ipc::IPC_ROUTER.lock().unregister_task(p1);
    crate::mmu::ArmMmu::switch_to_kernel();

    // 7. End-to-end reap_task_locksafe(): rejects while active -> preserves address_space in ReapFailed -> succeeds after deactivate()
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
        assert_ne!(aspace_ref.active_cpus(), 0);
        aspace_ref.deactivate();
    }
    assert_eq!(reap_task_locksafe(ls_pid.wrapping_add(100), true, ls_pid), Ok(0));
    {
        let mut g_sched = SCHEDULER.lock();
        assert_eq!(g_sched.tasks.len(), orig_len);
        g_sched.next_pid = orig_next_pid;
    }
    crate::mmu::ArmMmu::switch_to_kernel();
    crate::kprintln!("[TEST] AArch64 5-Stage Reap Evacuation Protocol VERIFIED.");
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
    test_reap_evacuation_protocol();
}
