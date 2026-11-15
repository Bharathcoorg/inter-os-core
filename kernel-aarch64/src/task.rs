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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::sync::IrqSpinLock as Mutex;

pub const STACK_SIZE: usize = 16384; // 16 KiB per kernel task stack
pub const MAX_CPUS: usize = 8;
pub const DEFAULT_TIME_SLICE: u8 = 4;

#[derive(Debug, Clone, Copy, Default)]
#[repr(C, align(16))]
pub struct ArmContext {
    /// Callee-saved general-purpose registers x19-x28: 10 * 8 = 80 bytes (offset 0)
    pub x19_x28: [u64; 10],
    /// Frame pointer x29 (FP): 8 bytes (offset 80)
    pub fp: u64,
    /// Link register x30 (LR) / cooperative return address: 8 bytes (offset 88)
    pub lr: u64,
    /// Dedicated stack pointer SP: 8 bytes (offset 96)
    pub sp: u64,
    /// Padding for 16-byte alignment of 128-bit vector registers: 8 bytes (offset 104)
    pub _pad: u64,
    /// Full 128-bit SIMD / FP vector register bank: q0-q31 = 32 * 16 = 512 bytes (offset 112)
    pub q: [u128; 32],
    /// Floating-Point Control Register: 8 bytes (offset 624)
    pub fpcr: u64,
    /// Floating-Point Status Register: 8 bytes (offset 632)
    pub fpsr: u64,
}

// Compile-time verification of cooperative task switch layout, total size (640 bytes),
// 16-byte alignment, and exact field offsets.
//
// NOTE: Asynchronous trap and exception handling (interrupts, SVC syscall dispatch, faults)
// uses `ExceptionFrame` in `exceptions.rs` with `eret`, `ELR_EL1`, `SPSR_EL1`, and `ESR_EL1`.
// `ArmContext` represents strictly the synchronous, cooperative AAPCS64 task context saved
// and restored across `switch_context` calls via standard `ret`.
const _: () = {
    assert!(core::mem::size_of::<ArmContext>() == 640);
    assert!(core::mem::align_of::<ArmContext>() == 16);
    assert!(core::mem::offset_of!(ArmContext, x19_x28) == 0);
    assert!(core::mem::offset_of!(ArmContext, fp) == 80);
    assert!(core::mem::offset_of!(ArmContext, lr) == 88);
    assert!(core::mem::offset_of!(ArmContext, sp) == 96);
    assert!(core::mem::offset_of!(ArmContext, _pad) == 104);
    assert!(core::mem::offset_of!(ArmContext, q) == 112);
    assert!(core::mem::offset_of!(ArmContext, fpcr) == 624);
    assert!(core::mem::offset_of!(ArmContext, fpsr) == 632);
};

impl ArmContext {
    pub const fn empty() -> Self {
        Self {
            x19_x28: [0u64; 10],
            fp: 0,
            lr: 0,
            sp: 0,
            _pad: 0,
            q: [0u128; 32],
            fpcr: 0,
            fpsr: 0,
        }
    }

    #[inline]
    pub fn lr(&self) -> u64 {
        self.lr
    }

    #[inline]
    pub fn set_lr(&mut self, val: u64) {
        self.lr = val;
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
#[no_mangle]
pub unsafe extern "C" fn switch_context(old_ctx: *mut ArmContext, new_ctx: *const ArmContext) {
    core::arch::naked_asm!(
        // Save callee-saved registers x19-x28 into old_ctx.x19_x28 (offset 0..80)
        "stp x19, x20, [x0, #0]",
        "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]",
        "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]",
        // Save frame pointer (x29) and link register (x30) (offset 80..96)
        "stp x29, x30, [x0, #80]",
        // Save current stack pointer (offset 96)
        "mov x2, sp",
        "str x2, [x0, #96]",
        // Save full 128-bit vector register bank q0-q31 (512 bytes at offset 112)
        "stp q0, q1, [x0, #112]",
        "stp q2, q3, [x0, #144]",
        "stp q4, q5, [x0, #176]",
        "stp q6, q7, [x0, #208]",
        "stp q8, q9, [x0, #240]",
        "stp q10, q11, [x0, #272]",
        "stp q12, q13, [x0, #304]",
        "stp q14, q15, [x0, #336]",
        "stp q16, q17, [x0, #368]",
        "stp q18, q19, [x0, #400]",
        "stp q20, q21, [x0, #432]",
        "stp q22, q23, [x0, #464]",
        "stp q24, q25, [x0, #496]",
        "stp q26, q27, [x0, #528]",
        "stp q28, q29, [x0, #560]",
        "stp q30, q31, [x0, #592]",
        // Save FPCR and FPSR floating point status/control (offset 624..640)
        "mrs x2, fpcr",
        "mrs x3, fpsr",
        "str x2, [x0, #624]",
        "str x3, [x0, #632]",

        // Restore callee-saved registers x19-x28 from new_ctx.x19_x28 (offset 0..80)
        "ldp x19, x20, [x1, #0]",
        "ldp x21, x22, [x1, #16]",
        "ldp x23, x24, [x1, #32]",
        "ldp x25, x26, [x1, #48]",
        "ldp x27, x28, [x1, #64]",
        // Restore frame pointer (x29) and link register (x30) (offset 80..96)
        "ldp x29, x30, [x1, #80]",
        // Restore stack pointer (offset 96)
        "ldr x2, [x1, #96]",
        "mov sp, x2",
        // Restore full 128-bit vector register bank q0-q31 (offset 112)
        "ldp q0, q1, [x1, #112]",
        "ldp q2, q3, [x1, #144]",
        "ldp q4, q5, [x1, #176]",
        "ldp q6, q7, [x1, #208]",
        "ldp q8, q9, [x1, #240]",
        "ldp q10, q11, [x1, #272]",
        "ldp q12, q13, [x1, #304]",
        "ldp q14, q15, [x1, #336]",
        "ldp q16, q17, [x1, #368]",
        "ldp q18, q19, [x1, #400]",
        "ldp q20, q21, [x1, #432]",
        "ldp q22, q23, [x1, #464]",
        "ldp q24, q25, [x1, #496]",
        "ldp q26, q27, [x1, #528]",
        "ldp q28, q29, [x1, #560]",
        "ldp q30, q31, [x1, #592]",
        // Restore FPCR and FPSR (offset 624..640)
        "ldr x2, [x1, #624]",
        "ldr x3, [x1, #632]",
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
    pub running_cpu: Option<usize>,
    pub switching_out: bool,
    pub affinity_mask: u32,
    pub time_slice_remaining: u8,
    pub last_cpu: Option<usize>,
    pub migrations: u64,
    pub kernel_entry_fn: Option<fn() -> !>,
}

impl TaskControlBlock {
    pub fn capabilities(&self) -> &CapabilityTable {
        &self.cap_table
    }

    pub fn capabilities_mut(&mut self) -> &mut CapabilityTable {
        &mut self.cap_table
    }

    #[inline]
    pub fn is_runnable_on(&self, cpu_id: usize) -> bool {
        if self.state != TaskState::Ready || self.switching_out || self.running_cpu.is_some() {
            return false;
        }
        if cpu_id >= MAX_CPUS {
            return false;
        }
        (self.affinity_mask & (1u32 << cpu_id)) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReapHandle {
    pub pid: u32,
    pub generation: u32,
    pub exit_code: i32,
}

#[repr(C, align(64))]
pub struct PerCpuData {
    pub logical_id: usize,
    pub idle_context: ArmContext,
    pub current_pid: AtomicU32,
    pub pending_release_pid: AtomicU32,
    pub need_resched: AtomicBool,
    pub tlb_shootdown_pending: AtomicBool,
    pub timer_ticks: AtomicU64,
    pub context_switches: AtomicU64,
    pub resched_ipis: AtomicU64,
    pub tlb_ipis: AtomicU64,
}

impl PerCpuData {
    pub const fn new(logical_id: usize) -> Self {
        Self {
            logical_id,
            idle_context: ArmContext::empty(),
            current_pid: AtomicU32::new(INVALID_PID),
            pending_release_pid: AtomicU32::new(INVALID_PID),
            need_resched: AtomicBool::new(false),
            tlb_shootdown_pending: AtomicBool::new(false),
            timer_ticks: AtomicU64::new(0),
            context_switches: AtomicU64::new(0),
            resched_ipis: AtomicU64::new(0),
            tlb_ipis: AtomicU64::new(0),
        }
    }
}

pub static mut PER_CPU_DATA: [PerCpuData; MAX_CPUS] = [
    PerCpuData::new(0),
    PerCpuData::new(1),
    PerCpuData::new(2),
    PerCpuData::new(3),
    PerCpuData::new(4),
    PerCpuData::new(5),
    PerCpuData::new(6),
    PerCpuData::new(7),
];

pub static SMP_SCHEDULING_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn set_smp_scheduling_enabled(enabled: bool) {
    SMP_SCHEDULING_ENABLED.store(enabled, Ordering::Release);
}

pub fn is_smp_scheduling_enabled() -> bool {
    SMP_SCHEDULING_ENABLED.load(Ordering::Acquire)
}

pub fn init_per_cpu(logical_id: usize) {
    let slot = if logical_id < MAX_CPUS { logical_id } else { 0 };
    unsafe {
        let ptr = &raw mut PER_CPU_DATA[slot];
        (*ptr).logical_id = slot;
        (*ptr).current_pid.store(INVALID_PID, Ordering::Release);
        (*ptr).pending_release_pid.store(INVALID_PID, Ordering::Release);
        (*ptr).need_resched.store(false, Ordering::Release);
        (*ptr).tlb_shootdown_pending.store(false, Ordering::Release);
        #[cfg(target_arch = "aarch64")]
        {
            core::arch::asm!("msr tpidr_el1, {0}", "isb", in(reg) (ptr as u64), options(nostack));
        }
    }
}

#[inline]
pub fn current_cpu_logical_id() -> usize {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        let tpidr: u64;
        core::arch::asm!("mrs {0}, tpidr_el1", out(reg) tpidr, options(nomem, nostack));
        let base = (&raw const PER_CPU_DATA[0]) as u64;
        let stride = core::mem::size_of::<PerCpuData>() as u64;
        let end = base + (MAX_CPUS as u64) * stride;
        if tpidr >= base && tpidr < end && (tpidr - base) % stride == 0 {
            let ptr = tpidr as *const PerCpuData;
            return (*ptr).logical_id.min(MAX_CPUS - 1);
        }
        0
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

#[inline]
pub fn per_cpu(logical_id: usize) -> &'static PerCpuData {
    let idx = if logical_id < MAX_CPUS { logical_id } else { 0 };
    unsafe { &*(&raw const PER_CPU_DATA[idx]) }
}

pub fn current_cpu_pid() -> u32 {
    let cid = current_cpu_logical_id();
    let pid = per_cpu(cid).current_pid.load(Ordering::Acquire);
    if !is_reserved_pid(pid) {
        return pid;
    }
    SCHEDULER.lock().current_pid()
}

pub struct Scheduler {
    pub tasks: Vec<Box<TaskControlBlock>>,
    pub current_task_idx: usize,
    pub next_pid: u32,
    pub rr_cursor: [usize; MAX_CPUS],
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
            rr_cursor: [0; MAX_CPUS],
        }
    }

    pub fn current_pid(&self) -> u32 {
        let cid = current_cpu_logical_id();
        let pid = per_cpu(cid).current_pid.load(Ordering::Acquire);
        if !is_reserved_pid(pid) && self.tasks.iter().any(|t| t.id == pid) {
            return pid;
        }
        if cid == 0 && self.current_task_idx < self.tasks.len() {
            self.tasks[self.current_task_idx].id
        } else {
            INVALID_PID
        }
    }

    pub fn current_task_pos(&self) -> Option<usize> {
        let cid = current_cpu_logical_id();
        let pid = per_cpu(cid).current_pid.load(Ordering::Acquire);
        if !is_reserved_pid(pid) {
            if let Some(pos) = self.tasks.iter().position(|t| t.id == pid) {
                return Some(pos);
            }
        }
        if cid == 0 && self.current_task_idx < self.tasks.len() {
            Some(self.current_task_idx)
        } else {
            None
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
        let removed = *self.tasks.remove(pos);
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
        self.spawn_kernel_task_with_options(name, entry_fn, cap_table, 0, 0xFFFF_FFFF);
    }

    pub fn spawn_kernel_task_with_options(
        &mut self,
        name: &'static str,
        entry_fn: fn() -> !,
        cap_table: CapabilityTable,
        priority: u8,
        affinity_mask: u32,
    ) -> u32 {
        let task_id = self.allocate_pid().expect("PID allocation failure during kernel daemon spawn");
        let stack = Box::new([0u8; STACK_SIZE]);

        let stack_top = (stack.as_ptr() as u64) + STACK_SIZE as u64;
        let aligned_sp = stack_top & !0x0F;

        let mut context = ArmContext::default();
        context.sp = aligned_sp;
        context.set_lr(kernel_task_trampoline as *const () as usize as u64);

        crate::ipc::IPC_ROUTER.lock().register_task(task_id);

        self.tasks.push(Box::new(TaskControlBlock {
            id: task_id,
            parent_id: 0,
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority,
            address_space: None,
            page_table: None,
            user_entry: 0,
            user_stack_top: 0,
            exit_code: None,
            generation: 1,
            is_service: false,
            service_id: None,
            service_generation: None,
            running_cpu: None,
            switching_out: false,
            affinity_mask,
            time_slice_remaining: DEFAULT_TIME_SLICE,
            last_cpu: None,
            migrations: 0,
            kernel_entry_fn: Some(entry_fn),
        }));
        task_id
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

        self.tasks.push(Box::new(TaskControlBlock {
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
            running_cpu: None,
            switching_out: false,
            affinity_mask: 0xFFFF_FFFF,
            time_slice_remaining: DEFAULT_TIME_SLICE,
            last_cpu: None,
            migrations: 0,
            kernel_entry_fn: None,
        }));
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
        self.tasks.push(Box::new(TaskControlBlock {
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
            running_cpu: None,
            switching_out: false,
            affinity_mask: 0xFFFF_FFFF,
            time_slice_remaining: DEFAULT_TIME_SLICE,
            last_cpu: None,
            migrations: 0,
            kernel_entry_fn: None,
        }));
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

        self.tasks.push(Box::new(TaskControlBlock {
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
            running_cpu: None,
            switching_out: false,
            affinity_mask: 0xFFFF_FFFF,
            time_slice_remaining: DEFAULT_TIME_SLICE,
            last_cpu: None,
            migrations: 0,
            kernel_entry_fn: None,
        }));
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

        if self.tasks[pos].switching_out || self.tasks[pos].running_cpu.is_some() {
            self.abort_reap(handle, None);
            return Err(-16); // -EBUSY: task is still executing or switching out on a CPU
        }

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

        if self.tasks[pos].switching_out || self.tasks[pos].running_cpu.is_some() {
            self.abort_reap(handle, None);
            return Err(-16); // -EBUSY: task kernel stack/context still active on a CPU
        }

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
        while (!self.tasks[next_idx].is_runnable_on(0)) && count < self.tasks.len() {
            next_idx = (next_idx + 1) % self.tasks.len();
            count += 1;
        }

        if curr_idx == next_idx || !self.tasks[next_idx].is_runnable_on(0) {
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

    /// Selects the best runnable task for `cpu_id` (priority + per-CPU round-robin),
    /// handling idle-to-task, task-to-task, and task-to-idle transitions safely.
    ///
    /// Outgoing tasks enter `switching_out = true` until `finish_post_switch()` completes
    /// after `switch_context`, preventing another CPU from stealing a task mid-save.
    pub fn prepare_switch_for_cpu(&mut self, cpu_id: usize) -> Option<(*mut ArmContext, *const ArmContext)> {
        if cpu_id >= MAX_CPUS {
            return None;
        }
        let pcpu = per_cpu(cpu_id);
        let curr_pid = pcpu.current_pid.load(Ordering::Acquire);
        let curr_pos = if !is_reserved_pid(curr_pid) {
            self.tasks.iter().position(|t| t.id == curr_pid)
        } else {
            None
        };

        let curr_still_runnable = curr_pos
            .map(|p| {
                self.tasks[p].state == TaskState::Running
                    && !self.tasks[p].switching_out
                    && (self.tasks[p].affinity_mask & (1u32 << cpu_id)) != 0
            })
            .unwrap_or(false);

        let n = self.tasks.len();
        let start = if n == 0 {
            0
        } else if let Some(c) = curr_pos {
            (c + 1) % n
        } else {
            self.rr_cursor[cpu_id] % n
        };

        let mut best_pos: Option<usize> = None;
        let mut best_prio: u8 = u8::MAX;
        for offset in 0..n {
            let idx = (start + offset) % n;
            let t = &self.tasks[idx];
            if t.is_runnable_on(cpu_id) && t.priority < best_prio {
                best_prio = t.priority;
                best_pos = Some(idx);
                if best_prio == 0 {
                    break;
                }
            }
        }

        // Don't switch away from a higher-priority task unless its timeslice hit 0 or it is no longer Running.
        if let (Some(cpos), Some(npos)) = (curr_pos, best_pos) {
            if curr_still_runnable
                && self.tasks[cpos].time_slice_remaining > 0
                && self.tasks[npos].priority > self.tasks[cpos].priority
            {
                best_pos = None;
            }
        }

        // If current task stopped running (Exited/Terminated/Blocked/Reaping) or idle has no runnable tasks:
        let next_pos = match (curr_pos, best_pos) {
            (_, Some(npos)) => Some(npos),
            (Some(cpos), None) => {
                // Keep running if still Running and permitted on this CPU; otherwise park onto idle_context.
                if curr_still_runnable {
                    self.tasks[cpos].time_slice_remaining = DEFAULT_TIME_SLICE;
                    return None;
                }
                None
            }
            (None, None) => return None,
        };

        // Transactional TTBR0 / UserAddressSpace switch BEFORE committing state transitions.
        if let Some(npos) = next_pos {
            if let Some(ref aspace) = self.tasks[npos].address_space {
                if let Err(e) = aspace.try_activate() {
                    crate::kprintln!("[SCHED CPU {}] Activation failed for PID {}: {:?}", cpu_id, self.tasks[npos].id, e);
                    return None;
                }
                if let Some(cpos) = curr_pos {
                    if let Some(ref out_aspace) = self.tasks[cpos].address_space {
                        out_aspace.deactivate_after_switch();
                    }
                }
            } else if let Some(cpos) = curr_pos {
                if let Some(ref out_aspace) = self.tasks[cpos].address_space {
                    out_aspace.deactivate();
                } else {
                    crate::mmu::ArmMmu::switch_to_kernel();
                }
            } else {
                crate::mmu::ArmMmu::switch_to_kernel();
            }
        } else if let Some(cpos) = curr_pos {
            if let Some(ref out_aspace) = self.tasks[cpos].address_space {
                out_aspace.deactivate();
            } else {
                crate::mmu::ArmMmu::switch_to_kernel();
            }
        }

        let old_ctx: *mut ArmContext = if let Some(cpos) = curr_pos {
            let out_task = &mut self.tasks[cpos];
            out_task.switching_out = true;
            out_task.running_cpu = Some(cpu_id);
            pcpu.pending_release_pid.store(out_task.id, Ordering::Release);
            &mut out_task.context as *mut ArmContext
        } else {
            pcpu.pending_release_pid.store(INVALID_PID, Ordering::Release);
            unsafe { &raw mut PER_CPU_DATA[cpu_id].idle_context }
        };

        let n_tasks = self.tasks.len().max(1);
        let new_ctx: *const ArmContext = if let Some(npos) = next_pos {
            let ctx_ptr = {
                let in_task = &mut self.tasks[npos];
                if let Some(prev_cpu) = in_task.last_cpu {
                    if prev_cpu != cpu_id {
                        in_task.migrations = in_task.migrations.saturating_add(1);
                    }
                }
                in_task.last_cpu = Some(cpu_id);
                in_task.state = TaskState::Running;
                in_task.running_cpu = Some(cpu_id);
                in_task.switching_out = false;
                in_task.time_slice_remaining = DEFAULT_TIME_SLICE;
                in_task.cpu_ticks += 1;
                pcpu.current_pid.store(in_task.id, Ordering::Release);
                (&in_task.context) as *const ArmContext
            };
            self.rr_cursor[cpu_id] = (npos + 1) % n_tasks;
            if cpu_id == 0 {
                self.current_task_idx = npos;
            }
            ctx_ptr
        } else {
            pcpu.current_pid.store(INVALID_PID, Ordering::Release);
            unsafe { &raw const PER_CPU_DATA[cpu_id].idle_context }
        };

        pcpu.context_switches.fetch_add(1, Ordering::Relaxed);
        Some((old_ctx, new_ctx))
    }

    pub fn complete_post_switch_for_cpu(&mut self, cpu_id: usize) {
        if cpu_id >= MAX_CPUS {
            return;
        }
        let rel_pid = per_cpu(cpu_id).pending_release_pid.swap(INVALID_PID, Ordering::AcqRel);
        if is_reserved_pid(rel_pid) {
            return;
        }
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == rel_pid) {
            t.switching_out = false;
            t.running_cpu = None;
            if t.state == TaskState::Running {
                t.state = TaskState::Ready;
            }
        }
    }
}

/// Releases ownership of any outgoing task that just switched off this CPU.
/// Must be called immediately after `switch_context` returns AND at the start
/// of every new task trampoline (`kernel_task_trampoline`, `user_process_entry`).
pub fn finish_post_switch() {
    let cid = current_cpu_logical_id();
    let mut sched = SCHEDULER.lock();
    sched.complete_post_switch_for_cpu(cid);
}

extern "C" fn kernel_task_trampoline() -> ! {
    finish_post_switch();
    let entry_opt = {
        let sched = SCHEDULER.lock();
        sched.current_task_pos().and_then(|pos| sched.tasks[pos].kernel_entry_fn)
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
    }
    if let Some(entry_fn) = entry_opt {
        entry_fn();
    }
    loop {
        yield_now();
    }
}

/// Called from `dispatch_irq` on PPI 27 timer interrupt.
pub fn on_timer_tick_irq() {
    let cid = current_cpu_logical_id();
    let pcpu = per_cpu(cid);
    pcpu.timer_ticks.fetch_add(1, Ordering::Relaxed);

    if !is_smp_scheduling_enabled() {
        return;
    }

    let mut sched = SCHEDULER.lock();
    let curr_pid = pcpu.current_pid.load(Ordering::Acquire);
    if is_reserved_pid(curr_pid) {
        if sched.tasks.iter().any(|t| t.is_runnable_on(cid)) {
            pcpu.need_resched.store(true, Ordering::Release);
        }
        return;
    }
    if let Some(pos) = sched.tasks.iter().position(|t| t.id == curr_pid) {
        sched.tasks[pos].cpu_ticks += 1;
        if sched.tasks[pos].state != TaskState::Running {
            pcpu.need_resched.store(true, Ordering::Release);
            return;
        }
        if sched.tasks[pos].time_slice_remaining > 1 {
            sched.tasks[pos].time_slice_remaining -= 1;
        } else {
            sched.tasks[pos].time_slice_remaining = 0;
            if sched.tasks.iter().any(|t| t.is_runnable_on(cid)) {
                pcpu.need_resched.store(true, Ordering::Release);
            }
        }
    } else {
        pcpu.need_resched.store(true, Ordering::Release);
    }
}

/// Called from `dispatch_irq` on SGI 0 (Reschedule IPI).
pub fn on_reschedule_ipi() {
    let cid = current_cpu_logical_id();
    let pcpu = per_cpu(cid);
    pcpu.resched_ipis.fetch_add(1, Ordering::Relaxed);
    if is_smp_scheduling_enabled() {
        pcpu.need_resched.store(true, Ordering::Release);
    }
}

/// Called from `dispatch_irq` on SGI 1 (TLB Shootdown IPI).
pub fn on_tlb_shootdown_ipi() {
    let cid = current_cpu_logical_id();
    let pcpu = per_cpu(cid);
    pcpu.tlb_ipis.fetch_add(1, Ordering::Relaxed);
    pcpu.tlb_shootdown_pending.store(false, Ordering::Release);
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb ishst", "tlbi vmalle1", "dsb ish", "isb", options(nostack));
    }
    if is_smp_scheduling_enabled() {
        let curr_pid = pcpu.current_pid.load(Ordering::Acquire);
        if !is_reserved_pid(curr_pid) {
            let sched = SCHEDULER.lock();
            if let Some(pos) = sched.tasks.iter().position(|t| t.id == curr_pid) {
                if let Some(ref aspace) = sched.tasks[pos].address_space {
                    if aspace.is_closing() {
                        aspace.deactivate();
                        pcpu.need_resched.store(true, Ordering::Release);
                    }
                }
            }
        }
    }
}

/// Performs preemptive task switching at the tail of an IRQ handler (after GIC EOI).
/// Runs with `DAIF.I = 1`.
pub fn preempt_from_irq() {
    if !is_smp_scheduling_enabled() {
        return;
    }
    let cid = current_cpu_logical_id();
    let pcpu = per_cpu(cid);
    if !pcpu.need_resched.swap(false, Ordering::AcqRel) {
        return;
    }
    let switch_pair = {
        let mut sched = SCHEDULER.lock();
        sched.prepare_switch_for_cpu(cid)
    };
    if let Some((old_ctx, new_ctx)) = switch_pair {
        unsafe {
            switch_context(old_ctx, new_ctx);
        }
        finish_post_switch();
    }
}

pub fn secondary_scheduler_loop(logical_id: usize) -> ! {
    loop {
        if is_smp_scheduling_enabled() {
            yield_now();
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("msr daifset, #2", options(nomem, nostack));
            let has_runnable = if is_smp_scheduling_enabled() {
                let sched = SCHEDULER.lock();
                sched.tasks.iter().any(|t| t.is_runnable_on(logical_id))
            } else {
                false
            };
            if !has_runnable && !per_cpu(logical_id).need_resched.load(Ordering::Acquire) {
                core::arch::asm!("dsb sy", "wfi", options(nomem, nostack));
            }
            core::arch::asm!("msr daifclr, #2", options(nomem, nostack));
        }
    }
}

pub fn reap_task_locksafe(caller_pid: u32, is_supervisor: bool, target_pid: u32) -> Result<i32, i64> {
    // Stage 1: Mark target Reaping under lock while keeping address_space attached.
    let handle = {
        let mut sched = SCHEDULER.lock();
        sched.begin_reap(caller_pid, is_supervisor, target_pid)?
    };

    // Stage 2: Bounded evacuation check outside SCHEDULER.lock() so remote CPUs
    // can acquire the lock, execute prepare_switch(), finish switch_context +
    // finish_post_switch(), and deactivate the outgoing address space before we
    // ever detach or free the task's stack/context/address space.
    const MAX_EVACUATION_POLLS: usize = 4_096;
    let mut evacuated = false;
    let caller_cid = current_cpu_logical_id();
    for poll in 0..MAX_EVACUATION_POLLS {
        let mut kick_cpu: Option<usize> = None;
        let check = {
            let sched = SCHEDULER.lock();
            match sched.tasks.iter().position(|t| {
                t.id == handle.pid && t.generation == handle.generation && t.state == TaskState::Reaping
            }) {
                Some(pos) => {
                    let t = &sched.tasks[pos];
                    let task_evacuated = !t.switching_out && t.running_cpu.is_none();
                    if !task_evacuated {
                        kick_cpu = t.running_cpu;
                    }
                    let aspace_evacuated = match t.address_space.as_ref() {
                        Some(aspace) => Scheduler::is_aspace_evacuated(aspace),
                        None => true,
                    };
                    Ok(task_evacuated && aspace_evacuated)
                }
                None => Err(-3i64),
            }
        };

        match check {
            Ok(true) => {
                evacuated = true;
                break;
            }
            Ok(false) => {
                if let Some(cpu) = kick_cpu {
                    per_cpu(cpu).need_resched.store(true, Ordering::Release);
                    if poll % 512 == 0 && cpu != caller_cid && is_smp_scheduling_enabled() {
                        crate::gic::broadcast_sgi_to_mask(0, 1u32 << cpu);
                    }
                }
                // Back off outside SCHEDULER.lock() so remote CPUs can acquire it to finish switch_out
                for _ in 0..256 {
                    core::hint::spin_loop();
                }
            }
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
    finish_post_switch();
    let (entry, sp) = {
        let sched = SCHEDULER.lock();
        let pos = sched.current_task_pos().unwrap_or(0);
        (sched.tasks[pos].user_entry, sched.tasks[pos].user_stack_top)
    };
    unsafe {
        crate::user::enter_scheduled_user_mode(entry, sp);
    }
}

pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

pub fn yield_now() {
    #[cfg(target_arch = "aarch64")]
    let saved_daif: u64;
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("mrs {0}, daif", out(reg) saved_daif, options(nomem, nostack));
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }

    let cid = current_cpu_logical_id();
    let switch_pair = {
        let mut sched = SCHEDULER.lock();
        if is_smp_scheduling_enabled() {
            sched.prepare_switch_for_cpu(cid)
        } else {
            let (o, n) = sched.prepare_switch();
            match (o, n) {
                (Some(old_c), Some(new_c)) => Some((old_c, new_c)),
                _ => None,
            }
        }
    };
    if let Some((old_ctx, new_ctx)) = switch_pair {
        unsafe {
            switch_context(old_ctx, new_ctx);
        }
        finish_post_switch();
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr daif, {0}", in(reg) saved_daif, options(nomem, nostack));
    }
}

#[no_mangle]
static mut TEST_SWITCH_FLAG: u32 = 0;
#[no_mangle]
static mut MAIN_CTX: ArmContext = ArmContext::empty();
#[no_mangle]
static mut WORKER_CTX: ArmContext = ArmContext::empty();
#[repr(align(16))]
#[allow(dead_code)]
struct WorkerStack([u8; 8192]);
static mut WORKER_STACK: WorkerStack = WorkerStack([0u8; 8192]);

core::arch::global_asm!(
    r#"
    .section .text
    .global asm_simd_test_runner
    .type asm_simd_test_runner, %function
    asm_simd_test_runner:
        // AAPCS64: allocate 128-byte stack frame (16-byte aligned)
        sub sp, sp, #128
        stp x29, x30, [sp, #0]
        stp x19, x20, [sp, #16]
        stp x21, x22, [sp, #32]
        stp x23, x24, [sp, #48]
        stp x25, x26, [sp, #64]
        stp x27, x28, [sp, #80]
        str x5, [sp, #96]   // destination pointer for captured_q
        str x6, [sp, #104]  // destination pointer for captured_gpr
        str x7, [sp, #112]  // destination pointer for captured_fp

        // Load test FPCR and FPSR from test_fp (in x4)
        ldr x8, [x4, #0]
        ldr x9, [x4, #8]
        msr fpcr, x8
        msr fpsr, x9

        // Load all 32 vector registers directly from patterns_q (in x2)
        ldp q0, q1, [x2, #0]
        ldp q2, q3, [x2, #32]
        ldp q4, q5, [x2, #64]
        ldp q6, q7, [x2, #96]
        ldp q8, q9, [x2, #128]
        ldp q10, q11, [x2, #160]
        ldp q12, q13, [x2, #192]
        ldp q14, q15, [x2, #224]
        ldp q16, q17, [x2, #256]
        ldp q18, q19, [x2, #288]
        ldp q20, q21, [x2, #320]
        ldp q22, q23, [x2, #352]
        ldp q24, q25, [x2, #384]
        ldp q26, q27, [x2, #416]
        ldp q28, q29, [x2, #448]
        ldp q30, q31, [x2, #480]

        // Load all 10 callee-saved GPRs x19-x28 from patterns_gpr (in x3)
        ldp x19, x20, [x3, #0]
        ldp x21, x22, [x3, #16]
        ldp x23, x24, [x3, #32]
        ldp x25, x26, [x3, #48]
        ldp x27, x28, [x3, #64]

        // Trigger cooperative switch_context: x0=main_ctx, x1=worker_ctx
        bl switch_context

        // =====================================================================
        // RESUMED IN TASK A (Original Task)
        // switch_context restored:
        // - x19-x28 from MAIN_CTX.x19_x28
        // - q0-q31 from MAIN_CTX.q
        // - fpcr, fpsr from MAIN_CTX.fpcr, fpsr
        // - sp from MAIN_CTX.sp
        //
        // ZERO compiler instructions or registers have intervened.
        // sp points to our 128-byte stack frame.
        // Immediately store restored state into caller buffers!
        // =====================================================================

        // 1. Capture restored callee-saved GPRs x19-x28 into captured_gpr buffer:
        ldr x0, [sp, #104]
        stp x19, x20, [x0, #0]
        stp x21, x22, [x0, #16]
        stp x23, x24, [x0, #32]
        stp x25, x26, [x0, #48]
        stp x27, x28, [x0, #64]

        // 2. Capture restored SIMD registers q0-q31 into captured_q buffer:
        ldr x0, [sp, #96]
        stp q0, q1, [x0, #0]
        stp q2, q3, [x0, #32]
        stp q4, q5, [x0, #64]
        stp q6, q7, [x0, #96]
        stp q8, q9, [x0, #128]
        stp q10, q11, [x0, #160]
        stp q12, q13, [x0, #192]
        stp q14, q15, [x0, #224]
        stp q16, q17, [x0, #256]
        stp q18, q19, [x0, #288]
        stp q20, q21, [x0, #320]
        stp q22, q23, [x0, #352]
        stp q24, q25, [x0, #384]
        stp q26, q27, [x0, #416]
        stp q28, q29, [x0, #448]
        stp q30, q31, [x0, #480]

        // 3. Capture restored FPCR and FPSR into captured_fp buffer:
        ldr x0, [sp, #112]
        mrs x1, fpcr
        mrs x2, fpsr
        stp x1, x2, [x0, #0]

        // 4. Restore caller's original callee-saved GPRs and return to Rust caller:
        ldp x19, x20, [sp, #16]
        ldp x21, x22, [sp, #32]
        ldp x23, x24, [sp, #48]
        ldp x25, x26, [sp, #64]
        ldp x27, x28, [sp, #80]
        ldp x29, x30, [sp, #0]
        add sp, sp, #128
        ret

    .global asm_test_worker_entry
    .type asm_test_worker_entry, %function
    asm_test_worker_entry:
        // Executing in Worker task context on WORKER_STACK.
        // 1. Deliberately clobber all 32 vector registers with poison value (0xDEADBEEF):
        mov x0, #0xBEEF
        movk x0, #0xDEAD, lsl #16
        movk x0, #0xBEEF, lsl #32
        movk x0, #0xDEAD, lsl #48
        dup v0.2d, x0
        dup v1.2d, x0
        dup v2.2d, x0
        dup v3.2d, x0
        dup v4.2d, x0
        dup v5.2d, x0
        dup v6.2d, x0
        dup v7.2d, x0
        dup v8.2d, x0
        dup v9.2d, x0
        dup v10.2d, x0
        dup v11.2d, x0
        dup v12.2d, x0
        dup v13.2d, x0
        dup v14.2d, x0
        dup v15.2d, x0
        dup v16.2d, x0
        dup v17.2d, x0
        dup v18.2d, x0
        dup v19.2d, x0
        dup v20.2d, x0
        dup v21.2d, x0
        dup v22.2d, x0
        dup v23.2d, x0
        dup v24.2d, x0
        dup v25.2d, x0
        dup v26.2d, x0
        dup v27.2d, x0
        dup v28.2d, x0
        dup v29.2d, x0
        dup v30.2d, x0
        dup v31.2d, x0

        // 2. Deliberately clobber all callee-saved GPRs x19-x28 with poison value (0xBAD0BAD0BAD0BAD0):
        mov x1, #0xBAD0
        movk x1, #0xBAD0, lsl #16
        movk x1, #0xBAD0, lsl #32
        movk x1, #0xBAD0, lsl #48
        mov x19, x1
        mov x20, x1
        mov x21, x1
        mov x22, x1
        mov x23, x1
        mov x24, x1
        mov x25, x1
        mov x26, x1
        mov x27, x1
        mov x28, x1

        // 3. Deliberately modify writable FPCR and FPSR:
        msr fpcr, xzr        // clear FZ, DN, RMode
        mov x2, #(1 << 3)    // set UFC (underflow cumulative exception), clear others
        msr fpsr, x2

        // 4. Set TEST_SWITCH_FLAG to 0xCAFEBABE
        adrp x0, TEST_SWITCH_FLAG
        add x0, x0, :lo12:TEST_SWITCH_FLAG
        mov x1, #0xBABE
        movk x1, #0xCAFE, lsl #16
        str w1, [x0]

        // 5. Switch back to Task A via production switch_context:
        // switch_context(old_ctx=&raw mut WORKER_CTX, new_ctx=&raw const MAIN_CTX)
        adrp x0, WORKER_CTX
        add x0, x0, :lo12:WORKER_CTX
        adrp x1, MAIN_CTX
        add x1, x1, :lo12:MAIN_CTX
        b switch_context
    "#
);

extern "C" {
    fn asm_simd_test_runner(
        main_ctx: *mut ArmContext,
        worker_ctx: *const ArmContext,
        patterns_q: *const u128,
        patterns_gpr: *const u64,
        test_fp: *const u64,
        captured_q: *mut u128,
        captured_gpr: *mut u64,
        captured_fp: *mut u64,
    );
    fn asm_test_worker_entry();
}

pub fn test_context_switching() {
    crate::kprintln!("--- [Testing AArch64 Callee-Saved Register Context Switcher] ---");

    unsafe {
        TEST_SWITCH_FLAG = 0;
        let stack_top = (core::ptr::addr_of!(WORKER_STACK) as u64) + 8192;
        assert_eq!(stack_top & 0x0F, 0, "Worker stack must be 16-byte aligned");

        // Set worker entry to pure assembly asm_test_worker_entry
        let mut wctx = core::ptr::read_volatile(core::ptr::addr_of!(WORKER_CTX));
        wctx.sp = stack_top;
        wctx.set_lr(asm_test_worker_entry as *const () as usize as u64);
        core::ptr::write_volatile(core::ptr::addr_of_mut!(WORKER_CTX), wctx);

        // 1. Distinguishable 128-bit test patterns across all 32 vector registers q0-q31
        let mut patterns_q = [0u128; 32];
        for i in 0..32 {
            patterns_q[i] = ((i as u128 + 1) << 96) | (0x0123_4567_89AB_CDEF_u128 ^ (i as u128));
        }

        // 2. Distinguishable 64-bit test patterns across all 10 callee-saved GPRs x19-x28
        let mut patterns_gpr = [0u64; 10];
        for i in 0..10 {
            patterns_gpr[i] = 0xAA19_0000_0000_0000_u64 | ((i as u64 + 19) << 32) | ((i as u64 + 1) * 0x1111);
        }

        // 3. Known architectural test values for FPCR and FPSR
        // FPCR: Bit 24 (FZ, Flush-to-zero), Bit 25 (DN, Default NaN), Bits 23:22 (RMode = 0b10, Round towards -Inf)
        let test_fpcr: u64 = (1 << 24) | (1 << 25) | (2 << 22);
        // FPSR: Bit 0 (IOC), Bit 1 (DZC), Bit 2 (OFC), Bit 28 (V), Bit 30 (Z), Bit 31 (N)
        let test_fpsr: u64 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 28) | (1 << 30) | (1 << 31);
        let test_fp = [test_fpcr, test_fpsr];

        let mut captured_q = [0u128; 32];
        let mut captured_gpr = [0u64; 10];
        let mut captured_fp = [0u64; 2]; // [fpcr, fpsr]

        // Execute assembly-first context switch test runner
        asm_simd_test_runner(
            core::ptr::addr_of_mut!(MAIN_CTX),
            core::ptr::addr_of!(WORKER_CTX),
            patterns_q.as_ptr(),
            patterns_gpr.as_ptr(),
            test_fp.as_ptr(),
            captured_q.as_mut_ptr(),
            captured_gpr.as_mut_ptr(),
            captured_fp.as_mut_ptr(),
        );

        // Verify that the worker executed on WORKER_STACK
        let flag_val = core::ptr::read_volatile(core::ptr::addr_of!(TEST_SWITCH_FLAG));
        assert_eq!(flag_val, 0xCAFE_BABE, "Context switch failed to execute worker task");

        // A. Verify that all 10 callee-saved GPRs (x19-x28) were restored bit-exact
        for i in 0..10 {
            assert_eq!(
                captured_gpr[i],
                patterns_gpr[i],
                "AArch64 GPR context switch restoration mismatch on register x{}",
                19 + i
            );
        }

        // B. Verify that all 32 vector registers (q0-q31) were restored bit-exact
        for i in 0..32 {
            assert_eq!(
                captured_q[i],
                patterns_q[i],
                "AArch64 SIMD context switch restoration mismatch on register q{}",
                i
            );
        }

        // C. Verify FPCR and FPSR architectural preservation
        // FPCR: test bits [25:22] (FZ, DN, RMode)
        let fpcr_mask: u64 = (1 << 24) | (1 << 25) | (3 << 22);
        assert_eq!(
            captured_fp[0] & fpcr_mask,
            test_fpcr & fpcr_mask,
            "FPCR register mismatch across context switch: expected 0x{:x}, got 0x{:x}",
            test_fpcr & fpcr_mask,
            captured_fp[0] & fpcr_mask
        );

        // FPSR: test cumulative exception bits [2:0] and condition flags [31:28]
        let fpsr_mask: u64 = (1 << 0) | (1 << 1) | (1 << 2) | (0xF << 28);
        assert_eq!(
            captured_fp[1] & fpsr_mask,
            test_fpsr & fpsr_mask,
            "FPSR register mismatch across context switch: expected 0x{:x}, got 0x{:x}",
            test_fpsr & fpsr_mask,
            captured_fp[1] & fpsr_mask
        );

        // D. Stack and control flow invariants
        let main_sp = core::ptr::read_volatile(core::ptr::addr_of!(MAIN_CTX.sp));
        let worker_sp = core::ptr::read_volatile(core::ptr::addr_of!(WORKER_CTX.sp));
        let main_lr = core::ptr::read_volatile(core::ptr::addr_of!(MAIN_CTX.lr));
        assert_eq!(main_sp & 0x0F, 0, "Main task stack pointer must remain 16-byte aligned");
        assert_eq!(worker_sp & 0x0F, 0, "Worker task stack pointer must remain 16-byte aligned");
        assert_ne!(main_lr, 0, "Saved return address (lr) must be non-zero");
    }

    crate::kprintln!("[TEST] AArch64 Context Switch: x19-x28 (GPRs), q0-q31 (SIMD), FPCR/FPSR, and sp/lr verified bit-exact.");
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
    let c_guard = sched.spawn_user_process(p1, "guarded_reap", 0, 0, Some(aspace_guard), caps.clone());
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

    // 8. Remote Unacknowledged CPU Rendezvous Rejection & Quarantine Safety
    let p_rem_parent = sched.spawn_user_process(0, "rem_parent", 0, 0, None, caps.clone());
    let aspace_remote = crate::mmu::UserAddressSpace::new().expect("Address space alloc for remote test");
    // Simulate remote CPU 1 executed but has not yet acknowledged
    aspace_remote.executed_cpus.fetch_or(1 << 1, core::sync::atomic::Ordering::Release);
    aspace_remote.acknowledged_cpus.fetch_and(!(1 << 1), core::sync::atomic::Ordering::Release);
    assert!(!Scheduler::is_aspace_evacuated(&aspace_remote), "Address space with unacknowledged remote CPU must not be evacuated");
    assert!(aspace_remote.has_unacknowledged_or_active_cpus(), "Must report unacknowledged remote CPU");

    let c_remote = sched.spawn_user_process(p_rem_parent, "remote_child", 0, 0, Some(aspace_remote), caps.clone());
    let c_rem_pos = sched.tasks.iter().position(|t| t.id == c_remote).unwrap();
    sched.tasks[c_rem_pos].state = TaskState::Terminated;

    let h_rem = sched.begin_reap(p_rem_parent, false, c_remote).expect("begin_reap on remote_child");
    assert!(matches!(sched.detach_reaping_aspace(h_rem), Err(-16)), "detach_reaping_aspace must reject while remote CPU unacknowledged");
    let c_rem_pos = sched.tasks.iter().position(|t| t.id == c_remote).unwrap();
    assert_eq!(sched.tasks[c_rem_pos].state, TaskState::ReapFailed);
    assert!(sched.tasks[c_rem_pos].address_space.is_some(), "address space must remain attached in ReapFailed");

    // Direct destroy_fallible() timeout and error check on unacknowledged remote CPU
    let mut unack_aspace = crate::mmu::UserAddressSpace::new().expect("Address space alloc");
    unack_aspace.executed_cpus.fetch_or(1 << 2, core::sync::atomic::Ordering::Release);
    unack_aspace.acknowledged_cpus.fetch_and(!(1 << 2), core::sync::atomic::Ordering::Release);
    assert_eq!(
        unack_aspace.destroy_fallible(),
        Err(crate::mmu::TeardownError::RemoteRendezvousFailed(1 << 2)),
        "destroy_fallible must time out with RemoteRendezvousFailed when remote CPU does not acknowledge"
    );
    // Acknowledge CPU 2 so unack_aspace can be cleanly destroyed without leaking frames
    unack_aspace.acknowledged_cpus.fetch_or(1 << 2, core::sync::atomic::Ordering::Release);
    assert!(unack_aspace.destroy_fallible().is_ok());

    // Clean up c_remote after acknowledging remote CPU 1
    {
        let c_rem_pos = sched.tasks.iter().position(|t| t.id == c_remote).unwrap();
        sched.tasks[c_rem_pos].address_space.as_ref().unwrap().acknowledged_cpus.fetch_or(1 << 1, core::sync::atomic::Ordering::Release);
        let h_clean = sched.begin_reap(p_rem_parent, false, c_remote).expect("begin_reap after remote acknowledge");
        let mut clean_aspace = sched.detach_reaping_aspace(h_clean).expect("detach clean").expect("aspace");
        assert!(clean_aspace.destroy_fallible().is_ok());
        assert_eq!(sched.finish_reap(h_clean), Ok(0));
    }
    crate::ipc::IPC_ROUTER.lock().unregister_task(p_rem_parent);

    // End-to-end reap_task_locksafe with remote unacknowledged CPU
    let (orig_len_rem, orig_next_pid_rem, rem_pid) = {
        let mut g_sched = SCHEDULER.lock();
        let len = g_sched.tasks.len();
        let next_p = g_sched.next_pid;
        let aspace_e2e = crate::mmu::UserAddressSpace::new().expect("Address space alloc for remote e2e");
        aspace_e2e.executed_cpus.fetch_or(1 << 1, core::sync::atomic::Ordering::Release);
        aspace_e2e.acknowledged_cpus.fetch_and(!(1 << 1), core::sync::atomic::Ordering::Release);
        let pid = g_sched.spawn_user_process(1, "remote_e2e", 0, 0, Some(aspace_e2e), CapabilityTable::new());
        if let Some(t) = g_sched.tasks.iter_mut().find(|t| t.id == pid) {
            t.state = TaskState::Terminated;
        }
        (len, next_p, pid)
    };
    assert_eq!(reap_task_locksafe(rem_pid.wrapping_add(100), true, rem_pid), Err(-16));
    {
        let g_sched = SCHEDULER.lock();
        let t = g_sched.tasks.iter().find(|t| t.id == rem_pid).expect("task must remain after remote EBUSY");
        assert_eq!(t.state, TaskState::ReapFailed);
        let aspace_ref = t.address_space.as_ref().expect("aspace must remain attached on remote EBUSY");
        assert!(!Scheduler::is_aspace_evacuated(aspace_ref));
        aspace_ref.acknowledged_cpus.fetch_or(1 << 1, core::sync::atomic::Ordering::Release);
        assert!(Scheduler::is_aspace_evacuated(aspace_ref));
    }
    assert_eq!(reap_task_locksafe(rem_pid.wrapping_add(100), true, rem_pid), Ok(0));
    {
        let mut g_sched = SCHEDULER.lock();
        assert_eq!(g_sched.tasks.len(), orig_len_rem);
        g_sched.next_pid = orig_next_pid_rem;
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
