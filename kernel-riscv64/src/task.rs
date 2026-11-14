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
#[repr(C, align(8))]
pub struct RiscvContext {
    /// Return address (x1 / ra): 8 bytes (offset 0)
    pub ra: u64,
    /// Stack pointer (x2 / sp): 8 bytes (offset 8)
    pub sp: u64,
    /// Frame pointer / callee-saved (x8 / s0): 8 bytes (offset 16)
    pub s0: u64,
    /// Callee-saved register (x9 / s1): 8 bytes (offset 24)
    pub s1: u64,
    /// Callee-saved registers (x18-x27 / s2-s11): 10 * 8 = 80 bytes (offset 32..112)
    pub s2_s11: [u64; 10],
    /// Full floating-point register file f0-f31 in standard RV64D/RV64F ABI: 32 * 8 = 256 bytes (offset 112..368)
    pub f: [u64; 32],
    /// Floating-point Control and Status Register FCSR: 8 bytes (offset 368..376)
    pub fcsr: u64,
}

// Compile-time verification of cooperative task switch layout, total size (376 bytes),
// 8-byte alignment, and exact field offsets.
//
// NOTE: Asynchronous trap and exception handling (interrupts, ecall syscall dispatch, page faults)
// uses `TrapFrame` in `trap.rs` with `sret`, `sepc`, `sstatus`, `scause`, and `stval`.
// `RiscvContext` represents strictly the synchronous, cooperative standard RISC-V ABI task context
// saved and restored across `switch_context` calls via standard `ret` (`jalr zero, 0(ra)`).
const _: () = {
    assert!(core::mem::size_of::<RiscvContext>() == 376);
    assert!(core::mem::align_of::<RiscvContext>() == 8);
    assert!(core::mem::offset_of!(RiscvContext, ra) == 0);
    assert!(core::mem::offset_of!(RiscvContext, sp) == 8);
    assert!(core::mem::offset_of!(RiscvContext, s0) == 16);
    assert!(core::mem::offset_of!(RiscvContext, s1) == 24);
    assert!(core::mem::offset_of!(RiscvContext, s2_s11) == 32);
    assert!(core::mem::offset_of!(RiscvContext, f) == 112);
    assert!(core::mem::offset_of!(RiscvContext, fcsr) == 368);
};

impl RiscvContext {
    pub const fn empty() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s0: 0,
            s1: 0,
            s2_s11: [0u64; 10],
            f: [0u64; 32],
            fcsr: 0,
        }
    }

    #[inline]
    pub fn ra(&self) -> u64 {
        self.ra
    }

    #[inline]
    pub fn set_ra(&mut self, val: u64) {
        self.ra = val;
    }

    #[inline]
    pub fn sp(&self) -> u64 {
        self.sp
    }

    #[inline]
    pub fn set_sp(&mut self, val: u64) {
        self.sp = val;
    }
}

/// Enables floating-point execution by setting `sstatus.FS` to Initial (0b01)
/// while ensuring Vector extension (`sstatus.VS` bits [10:9]) remains Off (0b00)
/// to strictly prevent silent vector state corruption.
pub fn init_fp() {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let mut sstatus: u64;
        core::arch::asm!("csrr {}, sstatus", out(reg) sstatus, options(nomem, nostack, preserves_flags));
        // Set FS bits [14:13] to Initial (0b01)
        sstatus &= !(3u64 << 13);
        sstatus |= 1u64 << 13;
        // Clear VS bits [10:9] to Off (0b00)
        sstatus &= !(3u64 << 9);
        core::arch::asm!("csrw sstatus, {}", in(reg) sstatus, options(nomem, nostack, preserves_flags));
    }
}

#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn switch_context(old_ctx: *mut RiscvContext, new_ctx: *const RiscvContext) {
    core::arch::naked_asm!(
        // a0 = old_ctx (*mut RiscvContext), a1 = new_ctx (*const RiscvContext)
        // 1. Save callee-saved GPRs to old_ctx (offset 0..112)
        "sd ra, 0(a0)",
        "sd sp, 8(a0)",
        "sd s0, 16(a0)",
        "sd s1, 24(a0)",
        "sd s2, 32(a0)",
        "sd s3, 40(a0)",
        "sd s4, 48(a0)",
        "sd s5, 56(a0)",
        "sd s6, 64(a0)",
        "sd s7, 72(a0)",
        "sd s8, 80(a0)",
        "sd s9, 88(a0)",
        "sd s10, 96(a0)",
        "sd s11, 104(a0)",

        // 2. Save full floating point register file f0-f31 (offset 112..368)
        "fsd f0, 112(a0)",
        "fsd f1, 120(a0)",
        "fsd f2, 128(a0)",
        "fsd f3, 136(a0)",
        "fsd f4, 144(a0)",
        "fsd f5, 152(a0)",
        "fsd f6, 160(a0)",
        "fsd f7, 168(a0)",
        "fsd f8, 176(a0)",
        "fsd f9, 184(a0)",
        "fsd f10, 192(a0)",
        "fsd f11, 200(a0)",
        "fsd f12, 208(a0)",
        "fsd f13, 216(a0)",
        "fsd f14, 224(a0)",
        "fsd f15, 232(a0)",
        "fsd f16, 240(a0)",
        "fsd f17, 248(a0)",
        "fsd f18, 256(a0)",
        "fsd f19, 264(a0)",
        "fsd f20, 272(a0)",
        "fsd f21, 280(a0)",
        "fsd f22, 288(a0)",
        "fsd f23, 296(a0)",
        "fsd f24, 304(a0)",
        "fsd f25, 312(a0)",
        "fsd f26, 320(a0)",
        "fsd f27, 328(a0)",
        "fsd f28, 336(a0)",
        "fsd f29, 344(a0)",
        "fsd f30, 352(a0)",
        "fsd f31, 360(a0)",

        // 3. Save FCSR floating-point status/control (offset 368)
        "frcsr t0",
        "sd t0, 368(a0)",

        // RESTORE from new_ctx (a1)
        // 1. Restore callee-saved GPRs (offset 0..112)
        "ld ra, 0(a1)",
        "ld sp, 8(a1)",
        "ld s0, 16(a1)",
        "ld s1, 24(a1)",
        "ld s2, 32(a1)",
        "ld s3, 40(a1)",
        "ld s4, 48(a1)",
        "ld s5, 56(a1)",
        "ld s6, 64(a1)",
        "ld s7, 72(a1)",
        "ld s8, 80(a1)",
        "ld s9, 88(a1)",
        "ld s10, 96(a1)",
        "ld s11, 104(a1)",

        // 2. Restore full floating point register file f0-f31 (offset 112..368)
        "fld f0, 112(a1)",
        "fld f1, 120(a1)",
        "fld f2, 128(a1)",
        "fld f3, 136(a1)",
        "fld f4, 144(a1)",
        "fld f5, 152(a1)",
        "fld f6, 160(a1)",
        "fld f7, 168(a1)",
        "fld f8, 176(a1)",
        "fld f9, 184(a1)",
        "fld f10, 192(a1)",
        "fld f11, 200(a1)",
        "fld f12, 208(a1)",
        "fld f13, 216(a1)",
        "fld f14, 224(a1)",
        "fld f15, 232(a1)",
        "fld f16, 240(a1)",
        "fld f17, 248(a1)",
        "fld f18, 256(a1)",
        "fld f19, 264(a1)",
        "fld f20, 272(a1)",
        "fld f21, 280(a1)",
        "fld f22, 288(a1)",
        "fld f23, 296(a1)",
        "fld f24, 304(a1)",
        "fld f25, 312(a1)",
        "fld f26, 320(a1)",
        "fld f27, 328(a1)",
        "fld f28, 336(a1)",
        "fld f29, 344(a1)",
        "fld f30, 352(a1)",
        "fld f31, 360(a1)",

        // 3. Restore FCSR (offset 368)
        "ld t0, 368(a1)",
        "fscsr t0",

        // 4. Return to restored return address (ra)
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
pub const REAP_EVACUATION_MAX_SPINS: usize = 1000;

pub fn is_reserved_pid(pid: u32) -> bool {
    pid == 0 || pid == SUPERVISOR_PID || pid == INVALID_PID
}

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
        if is_reserved_pid(_id.0 as u32) {
            return false;
        }
        let task_id = _id.0 as u32;
        if self.tasks.iter().any(|t| t.id == task_id) {
            return false; // Prevent duplicate task registration
        }
        if self.next_pid <= task_id {
            self.next_pid = task_id.wrapping_add(1);
            if is_reserved_pid(self.next_pid) {
                self.next_pid = 1;
            }
        }
        let stack = Box::new([0u8; STACK_SIZE]);
        let mut context = RiscvContext::default();
        context.set_sp(kstack_top as u64);
        context.set_ra(user_process_entry as *const () as usize as u64);

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
            is_service: false,
            service_id: None,
            service_generation: None,
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
        let task_id = match self.allocate_pid() {
            Some(id) => id,
            None => return 0,
        };
        let stack = Box::new([0u8; STACK_SIZE]);

        let kernel_sp = ((stack.as_ptr() as usize) + STACK_SIZE) & !0x0F;

        let mut context = RiscvContext::default();
        context.set_sp(kernel_sp as u64);
        context.set_ra(user_process_entry as *const () as usize as u64);

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
            is_service: false,
            service_id: None,
            service_generation: None,
        });
        task_id
    }

    pub(crate) fn spawn_service_process(
        &mut self,
        service_id: u8,
        generation: u32,
        name: &'static str,
        entry_point: usize,
        user_sp: usize,
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

        let kernel_sp = ((stack.as_ptr() as usize) + STACK_SIZE) & !0x0F;

        let mut context = RiscvContext::default();
        context.set_sp(kernel_sp as u64);
        context.set_ra(user_process_entry as *const () as usize as u64);

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        let satp = address_space.as_ref().map(|a| a.satp);

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
            page_table: satp,
            user_entry: entry_point,
            user_stack_top: user_sp,
            kstack_top: kernel_sp,
            exit_code: None,
            generation: 1,
            is_service: true,
            service_id: Some(service_id),
            service_generation: Some(generation),
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

#[no_mangle]
static mut TEST_SWITCH_FLAG: u32 = 0;
#[no_mangle]
static mut MAIN_CTX: RiscvContext = RiscvContext::empty();
#[no_mangle]
static mut WORKER_CTX: RiscvContext = RiscvContext::empty();
static mut WORKER_STACK: [u8; 8192] = [0u8; 8192];

core::arch::global_asm!(
    r#"
    .section .rodata
    .balign 8
poison_f_const:
    .quad 0xDEADBEEFCAFEBABE
poison_s_const:
    .quad 0xBAD0BAD0BAD0BAD0

    .section .text
    .global asm_riscv_context_test_runner
    .type asm_riscv_context_test_runner, @function
asm_riscv_context_test_runner:
    // a0: main_ctx (*mut RiscvContext)
    // a1: worker_ctx (*const RiscvContext)
    // a2: patterns_f (*const u64)
    // a3: patterns_s (*const u64)
    // a4: captured_f (*mut u64)
    // a5: captured_s (*mut u64)
    // a6: captured_fcsr (*mut u64)

    // 1. Allocate 128-byte stack frame (16-byte aligned)
    addi sp, sp, -128
    sd ra, 0(sp)
    sd a4, 8(sp)
    sd a5, 16(sp)
    sd a6, 24(sp)

    // Save caller's callee-saved s0-s11
    sd s0, 32(sp)
    sd s1, 40(sp)
    sd s2, 48(sp)
    sd s3, 56(sp)
    sd s4, 64(sp)
    sd s5, 72(sp)
    sd s6, 80(sp)
    sd s7, 88(sp)
    sd s8, 96(sp)
    sd s9, 104(sp)
    sd s10, 112(sp)
    sd s11, 120(sp)

    // 2. Load distinguishable test patterns into s0-s11 from a3
    ld s0, 0(a3)
    ld s1, 8(a3)
    ld s2, 16(a3)
    ld s3, 24(a3)
    ld s4, 32(a3)
    ld s5, 40(a3)
    ld s6, 48(a3)
    ld s7, 56(a3)
    ld s8, 64(a3)
    ld s9, 72(a3)
    ld s10, 80(a3)
    ld s11, 88(a3)

    // 3. Load distinguishable test patterns into f0-f31 from a2
    fld f0, 0(a2)
    fld f1, 8(a2)
    fld f2, 16(a2)
    fld f3, 24(a2)
    fld f4, 32(a2)
    fld f5, 40(a2)
    fld f6, 48(a2)
    fld f7, 56(a2)
    fld f8, 64(a2)
    fld f9, 72(a2)
    fld f10, 80(a2)
    fld f11, 88(a2)
    fld f12, 96(a2)
    fld f13, 104(a2)
    fld f14, 112(a2)
    fld f15, 120(a2)
    fld f16, 128(a2)
    fld f17, 136(a2)
    fld f18, 144(a2)
    fld f19, 152(a2)
    fld f20, 160(a2)
    fld f21, 168(a2)
    fld f22, 176(a2)
    fld f23, 184(a2)
    fld f24, 192(a2)
    fld f25, 200(a2)
    fld f26, 208(a2)
    fld f27, 216(a2)
    fld f28, 224(a2)
    fld f29, 232(a2)
    fld f30, 240(a2)
    fld f31, 248(a2)

    // 4. Trigger cooperative context switch: a0=main_ctx, a1=worker_ctx
    call switch_context

    // =====================================================================
    // RESUMED IN TASK A (Original Task)
    // Immediately capture all 32 floating-point registers f0-f31 into captured_f.
    // Zero compiler instructions or register clobbers intervene before this capture.
    // =====================================================================
    ld t1, 8(sp)
    fsd f0, 0(t1)
    fsd f1, 8(t1)
    fsd f2, 16(t1)
    fsd f3, 24(t1)
    fsd f4, 32(t1)
    fsd f5, 40(t1)
    fsd f6, 48(t1)
    fsd f7, 56(t1)
    fsd f8, 64(t1)
    fsd f9, 72(t1)
    fsd f10, 80(t1)
    fsd f11, 88(t1)
    fsd f12, 96(t1)
    fsd f13, 104(t1)
    fsd f14, 112(t1)
    fsd f15, 120(t1)
    fsd f16, 128(t1)
    fsd f17, 136(t1)
    fsd f18, 144(t1)
    fsd f19, 152(t1)
    fsd f20, 160(t1)
    fsd f21, 168(t1)
    fsd f22, 176(t1)
    fsd f23, 184(t1)
    fsd f24, 192(t1)
    fsd f25, 200(t1)
    fsd f26, 208(t1)
    fsd f27, 216(t1)
    fsd f28, 224(t1)
    fsd f29, 232(t1)
    fsd f30, 240(t1)
    fsd f31, 248(t1)

    // Capture callee-saved GPRs s0-s11 into captured_s
    ld t1, 16(sp)
    sd s0, 0(t1)
    sd s1, 8(t1)
    sd s2, 16(t1)
    sd s3, 24(t1)
    sd s4, 32(t1)
    sd s5, 40(t1)
    sd s6, 48(t1)
    sd s7, 56(t1)
    sd s8, 64(t1)
    sd s9, 72(t1)
    sd s10, 80(t1)
    sd s11, 88(t1)

    // Capture restored FCSR
    frcsr t0
    ld t1, 24(sp)
    sd t0, 0(t1)

    // Restore caller's callee-saved s0-s11 and return
    ld s0, 32(sp)
    ld s1, 40(sp)
    ld s2, 48(sp)
    ld s3, 56(sp)
    ld s4, 64(sp)
    ld s5, 72(sp)
    ld s6, 80(sp)
    ld s7, 88(sp)
    ld s8, 96(sp)
    ld s9, 104(sp)
    ld s10, 112(sp)
    ld s11, 120(sp)
    ld ra, 0(sp)
    addi sp, sp, 128
    ret

    .global asm_riscv_test_worker_entry
    .type asm_riscv_test_worker_entry, @function
asm_riscv_test_worker_entry:
    // Executing on WORKER_STACK in Worker task context.
    // Deliberately poison all f0-f31 with distinctive poison pattern
    la t1, poison_f_const
    ld t0, 0(t1)
    fmv.d.x f0, t0
    fmv.d.x f1, t0
    fmv.d.x f2, t0
    fmv.d.x f3, t0
    fmv.d.x f4, t0
    fmv.d.x f5, t0
    fmv.d.x f6, t0
    fmv.d.x f7, t0
    fmv.d.x f8, t0
    fmv.d.x f9, t0
    fmv.d.x f10, t0
    fmv.d.x f11, t0
    fmv.d.x f12, t0
    fmv.d.x f13, t0
    fmv.d.x f14, t0
    fmv.d.x f15, t0
    fmv.d.x f16, t0
    fmv.d.x f17, t0
    fmv.d.x f18, t0
    fmv.d.x f19, t0
    fmv.d.x f20, t0
    fmv.d.x f21, t0
    fmv.d.x f22, t0
    fmv.d.x f23, t0
    fmv.d.x f24, t0
    fmv.d.x f25, t0
    fmv.d.x f26, t0
    fmv.d.x f27, t0
    fmv.d.x f28, t0
    fmv.d.x f29, t0
    fmv.d.x f30, t0
    fmv.d.x f31, t0

    // Deliberately poison s0-s11
    la t1, poison_s_const
    ld t2, 0(t1)
    mv s0, t2
    mv s1, t2
    mv s2, t2
    mv s3, t2
    mv s4, t2
    mv s5, t2
    mv s6, t2
    mv s7, t2
    mv s8, t2
    mv s9, t2
    mv s10, t2
    mv s11, t2

    // Set TEST_SWITCH_FLAG = 0xCAFEBABE
    la t4, TEST_SWITCH_FLAG
    li t5, 0xCAFEBABE
    sw t5, 0(t4)

    // Switch back to Task A:
    // switch_context(old_ctx=&raw mut WORKER_CTX, new_ctx=&raw const MAIN_CTX)
    la a0, WORKER_CTX
    la a1, MAIN_CTX
    tail switch_context
    "#
);

extern "C" {
    fn asm_riscv_context_test_runner(
        main_ctx: *mut RiscvContext,
        worker_ctx: *const RiscvContext,
        patterns_f: *const u64,
        patterns_s: *const u64,
        captured_f: *mut u64,
        captured_s: *mut u64,
        captured_fcsr: *mut u64,
    );
    fn asm_riscv_test_worker_entry();
}

pub fn test_context_switching() {
    crate::println!("--- [Testing RISC-V Callee-Saved Register Context Switcher] ---");

    unsafe {
        TEST_SWITCH_FLAG = 0;
        let stack_top = (core::ptr::addr_of!(WORKER_STACK) as u64) + 8192;
        let mut wctx = WORKER_CTX;
        wctx.sp = stack_top & !0x0F;
        wctx.set_ra(asm_riscv_test_worker_entry as *const () as usize as u64);
        WORKER_CTX = wctx;

        let mut patterns_f = [0u64; 32];
        for i in 0..32 {
            patterns_f[i] = 0x1122_3344_5566_7700_u64 | (i as u64);
        }

        let mut patterns_s = [0u64; 12];
        for i in 0..12 {
            patterns_s[i] = 0xAA55_BB66_CC77_0000_u64 | ((i + 1) as u64);
        }

        let mut captured_f = [0u64; 32];
        let mut captured_s = [0u64; 12];
        let mut captured_fcsr = [0u64; 1];

        let mut orig_fcsr: u64;
        core::arch::asm!("frcsr {}", out(reg) orig_fcsr);

        asm_riscv_context_test_runner(
            core::ptr::addr_of_mut!(MAIN_CTX),
            core::ptr::addr_of!(WORKER_CTX),
            patterns_f.as_ptr(),
            patterns_s.as_ptr(),
            captured_f.as_mut_ptr(),
            captured_s.as_mut_ptr(),
            captured_fcsr.as_mut_ptr(),
        );

        let flag_val = core::ptr::read_volatile(core::ptr::addr_of!(TEST_SWITCH_FLAG));
        assert_eq!(flag_val, 0xCAFE_BABE, "RISC-V Context switch failed to execute worker task");

        for i in 0..32 {
            assert_eq!(
                captured_f[i],
                patterns_f[i],
                "RISC-V floating-point context switch restoration mismatch on f register"
            );
        }

        for i in 0..12 {
            assert_eq!(
                captured_s[i],
                patterns_s[i],
                "RISC-V callee-saved GPR context switch restoration mismatch on s register"
            );
        }

        assert_eq!(captured_fcsr[0], orig_fcsr, "RISC-V FCSR register mismatch across context switch");
    }

    crate::println!("[TEST] RISC-V Context Switch: ra, sp, s0-s11, all 32 floating-point registers (f0-f31), and FCSR saved & restored VERIFIED.");
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
    let svc_child = sched2.spawn_service_process(3, 1, "svc_child", 0, 0, None, caps.clone());
    let svc_pos = sched2.tasks.iter().position(|t| t.id == svc_child).unwrap();
    sched2.tasks[svc_pos].state = TaskState::Terminated;
    // Supervisor reap of svc_child succeeds using internal SUPERVISOR_PID
    let h_svc = sched2.begin_reap(SUPERVISOR_PID, true, svc_child).expect("Supervisor begin_reap on service task");
    assert_eq!(h_svc.pid, svc_child);
    assert_eq!(sched2.finish_reap(h_svc), Ok(0));

    crate::println!("[TEST] RISC-V Task Removal Consistency, 5-Stage Reap & Transactional Switch VERIFIED.");
}
