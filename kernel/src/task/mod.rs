// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS Multitasking & Process Foundation
//!
//! Provides the core primitives for threads and processes: `TaskId`,
//! `TaskState`, `TaskControlBlock` (TCB), stack management, and scheduling.

pub mod context;
pub mod futex;
pub mod images;
pub mod process;
pub mod scheduler;
pub mod user;

use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

pub use context::{switch_context, TaskContext};
pub use process::{ProcessId, ProcessRecord, ProcessState, ResourceLimits};
pub use scheduler::{on_timer_tick, terminate_current, yield_now, Scheduler, SCHEDULER};
pub use images::{
    USER_INIT_ELF, APP_SETTINGS_ELF, COMPOSITOR_ELF, LAUNCHER_ELF,
    APP_DIALER_ELF, APP_VAULT_ELF, APP_CAMERA_ELF, APP_CALCULATOR_ELF, APP_BROWSER_ELF,
    APP_MESSAGES_ELF, APP_CALENDAR_ELF, APP_MUSIC_ELF, APP_FILE_MANAGER_ELF, APP_GALLERY_ELF,
};
pub use user::{
    spawn_user_init,
    spawn_app_settings,
    spawn_app_dialer,
    spawn_app_vault,
    spawn_app_camera,
    spawn_app_calculator,
    spawn_app_browser,
    spawn_app_messages,
    spawn_app_calendar,
    spawn_app_music,
    spawn_packaged_process,
};
use crate::capability::CapabilityTable;

/// Size of the dedicated kernel stack allocated per task (128 KiB).
pub const TASK_STACK_SIZE: usize = 128 * 1024;

/// Stack guard canary word placed at the bottom of the stack to detect overflows.
pub const STACK_CANARY: u64 = 0xDEAD_BEEF_C0DE_CAFE;

/// Maximum number of detached kernel stacks to hold in the recycling pool.
/// Bounded to avoid unbounded cache expansion while eliminating stack-driven
/// allocator fragmentation across repeated process spawn and exit cycles.
const STACK_POOL_CAPACITY: usize = 4;

static STACK_POOL: Mutex<alloc::vec::Vec<Box<[u8]>>> = Mutex::new(alloc::vec::Vec::new());

/// Dedicated kernel stack memory for a task.
/// Automatically recycled into `STACK_POOL` on drop when capacity permits,
/// avoiding repeated 128 KiB heap allocations and allocator churn during lifecycle stress.
pub struct KernelStack {
    inner: Option<Box<[u8]>>,
}

impl core::fmt::Debug for KernelStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "KernelStack({:p}, len={})", self.as_ptr(), self.len())
    }
}

impl KernelStack {
    /// Creates an empty kernel stack descriptor (used for primordial bootstrap thread).
    pub const fn empty() -> Self {
        Self { inner: None }
    }

    /// Allocates a dedicated kernel stack, popping from `STACK_POOL` if available.
    pub fn allocate() -> Self {
        let recycled = {
            if let Some(mut pool) = STACK_POOL.try_lock() {
                pool.pop()
            } else {
                None
            }
        };

        let stack = if let Some(mut s) = recycled {
            if s.len() == TASK_STACK_SIZE {
                for b in s.iter_mut() {
                    *b = 0;
                }
                s
            } else {
                Self::create_fresh_stack()
            }
        } else {
            Self::create_fresh_stack()
        };

        Self { inner: Some(stack) }
    }

    fn create_fresh_stack() -> Box<[u8]> {
        let mut stack = Box::new_uninit_slice(TASK_STACK_SIZE);
        unsafe {
            for byte in stack.iter_mut() {
                byte.write(0);
            }
            stack.assume_init()
        }
    }
}

impl core::ops::Deref for KernelStack {
    type Target = [u8];
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        match &self.inner {
            Some(s) => s.as_ref(),
            None => &[],
        }
    }
}

impl core::ops::DerefMut for KernelStack {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.inner {
            Some(s) => s.as_mut(),
            None => &mut [],
        }
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        if let Some(s) = self.inner.take() {
            if s.len() == TASK_STACK_SIZE {
                if let Some(mut pool) = STACK_POOL.try_lock() {
                    if pool.len() < STACK_POOL_CAPACITY {
                        pool.push(s);
                    }
                }
            }
        }
    }
}

/// Strongly typed task identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub u64);

impl TaskId {
    /// Generates a globally unique TaskId.
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Scheduling class determining CPU execution guarantees and priority behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingClass {
    /// Normal priority-based time-sliced round-robin scheduling.
    Normal,
    /// Earliest-Deadline-First Real-Time class for ultra-low-latency UI/VSync pipelines.
    RealTime,
}

/// Execution state of a task in the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Ready to run, waiting for CPU time slice in run queue.
    Ready,
    /// Currently executing on CPU.
    Running,
    /// Blocked awaiting an external event, IPC message, or timer tick.
    Blocked,
    /// Blocked waiting for a child process to exit (waitpid).
    WaitPid(u64),
    /// Terminated but awaiting parent to reap exit status (zombie).
    Zombie,
    /// Terminated and fully reaped, awaiting resource reclamation.
    Terminated,
}

impl TaskState {
    pub const fn code(&self) -> u8 {
        match self {
            TaskState::Ready => 0,
            TaskState::Running => 1,
            TaskState::Blocked => 2,
            TaskState::WaitPid(_) => 3,
            TaskState::Zombie => 4,
            TaskState::Terminated => 5,
        }
    }
}

/// Enumerable task record for user-space system queries.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskRecord {
    pub task_id: u64,
    pub name: [u8; 16],
    pub name_len: u8,
    pub state: u8, // 0=Ready, 1=Running, 2=Blocked
    pub _reserved: [u8; 6],
    pub cpu_ticks: u64,
    pub mem_bytes: u64,
}

/// Enumerates up to `out.len()` active tasks from the scheduler.
pub fn enumerate_tasks(out: &mut [TaskRecord]) -> usize {
    let mut count = 0;
    if let Some(scheduler) = SCHEDULER.try_lock() {
        for tcb in scheduler.tasks_iter() {
            if count >= out.len() {
                break;
            }
            let mut name_buf = [0u8; 16];
            let name_bytes = tcb.name.as_bytes();
            let nlen = name_bytes.len().min(16);
            name_buf[..nlen].copy_from_slice(&name_bytes[..nlen]);

            let state_code = match tcb.state {
                TaskState::Ready => 0,
                TaskState::Running => 1,
                _ => 2,
            };

            let mem_bytes = if let Some(ref pt_arc) = tcb.page_table {
                if let Some(pt) = pt_arc.try_lock() {
                    if pt.user_mapped_bytes > 0 {
                        pt.user_mapped_bytes as u64
                    } else {
                        (pt.allocated_frames.len() as u64).saturating_mul(4096)
                    }
                } else {
                    0
                }
            } else {
                0
            } + (if !tcb.stack.is_empty() { TASK_STACK_SIZE as u64 } else { 0 });

            out[count] = TaskRecord {
                task_id: tcb.id.0,
                name: name_buf,
                name_len: nlen as u8,
                state: state_code,
                _reserved: [0; 6],
                cpu_ticks: tcb.cpu_ticks,
                mem_bytes,
            };
            count += 1;
        }
    }
    count
}

/// Task Control Block (TCB) maintaining complete execution state and security capabilities.
pub type SharedAddressSpace = Arc<Mutex<crate::memory::TaskPageTable>>;

pub struct TaskControlBlock {
    /// Unique task ID.
    pub id: TaskId,
    /// Human-readable diagnostic name for the task.
    pub name: &'static str,
    /// Current scheduling state.
    pub state: TaskState,
    /// Preserved hardware register context (stack pointer).
    pub context: TaskContext,
    /// Dedicated kernel stack memory.
    pub stack: KernelStack,
    /// Capability table holding guarded access tokens.
    pub cap_table: CapabilityTable,
    /// Total CPU timer ticks consumed by this task.
    pub cpu_ticks: u64,
    /// Priority level (higher numbers run with precedence).
    pub priority: u8,
    /// Base priority before any priority inheritance elevation.
    pub base_priority: u8,
    /// Active priority inheritance boosts count.
    pub priority_boosts: u8,
    /// Scheduling class (Normal or RealTime EDF).
    pub sched_class: SchedulingClass,
    /// Absolute deadline in timer ticks for EDF dispatch. 0 means unspecified.
    pub deadline_ticks: u64,
    /// Remaining frame budget in nanoseconds (<8.3ms for 120Hz, <16.6ms for 60Hz).
    pub frame_budget_ns: u64,
    /// Dedicated hardware virtual memory address space (CR3 Page Table).
    pub page_table: Option<SharedAddressSpace>,
    /// Cached physical frame for CR3 switch, accessible without spinlock contention.
    pub cr3_frame: Option<x86_64::structures::paging::PhysFrame<x86_64::structures::paging::Size4KiB>>,
    /// Virtual address of userspace entry point (Ring 3 RIP).
    pub user_entry: usize,
    /// Virtual address of userspace stack top (Ring 3 RSP).
    pub user_stack_top: usize,
    /// Thread-Local Storage (TLS) base address loaded into IA32_FS_BASE MSR on dispatch.
    pub tls_base: u64,
    /// User argument passed in RDI to thread entry point.
    pub user_arg: u64,
    /// Stable process identity owning this task. A process may later own multiple threads.
    pub process_id: Option<ProcessId>,
    /// Parent task ID (0 = init / no parent).
    pub parent_id: TaskId,
    /// Exit code set on termination (only valid in Zombie/Terminated state).
    pub exit_code: i32,
}

impl TaskControlBlock {
    /// Creates a new Task Control Block with an allocated stack and initial context.
    pub fn new(
        name: &'static str,
        entry_point: extern "C" fn() -> !,
        cap_table: CapabilityTable,
        priority: u8,
    ) -> Self {
        let mut stack = KernelStack::allocate();

        // Write canary at the bottom of the stack
        unsafe {
            let canary_ptr = stack.as_mut_ptr() as *mut u64;
            *canary_ptr = STACK_CANARY;
        }

        // Initialize stack frame for context switching
        let context = unsafe { TaskContext::create_initial(&mut stack, entry_point) };

        let id = TaskId::new();
        let mut cap_table = cap_table;
        cap_table.set_owner_task(id);

        Self {
            id,
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            base_priority: priority,
            priority,
            priority_boosts: 0,
            sched_class: SchedulingClass::Normal,
            deadline_ticks: 0,
            frame_budget_ns: 16_666_666,
            page_table: None,
            cr3_frame: None,
            user_entry: 0,
            user_stack_top: 0,
            tls_base: 0,
            user_arg: 0,
            process_id: None,
            parent_id: TaskId(0),
            exit_code: 0,
        }
    }

    /// Attaches an isolated hardware page table to this task.
    pub fn with_page_table(mut self, pt: crate::memory::TaskPageTable) -> Self {
        self.cr3_frame = Some(pt.frame());
        self.page_table = Some(Arc::new(Mutex::new(pt)));
        self
    }

    /// Creates an existing/current task representation (for the bootstrap main thread).
    pub fn from_current(name: &'static str, cap_table: CapabilityTable) -> Self {
        let mut cap_table = cap_table;
        cap_table.set_owner_task(TaskId(0));

        Self {
            id: TaskId(0), // Task 0 is the primordial kernel main thread
            name,
            state: TaskState::Running,
            context: TaskContext::empty(),
            stack: KernelStack::empty(),
            cap_table,
            cpu_ticks: 0,
            base_priority: 0,
            priority: 0,
            priority_boosts: 0,
            sched_class: SchedulingClass::Normal,
            deadline_ticks: 0,
            frame_budget_ns: 16_666_666,
            page_table: None,
            cr3_frame: None,
            user_entry: 0,
            user_stack_top: 0,
            tls_base: 0,
            user_arg: 0,
            process_id: None,
            parent_id: TaskId(0),
            exit_code: 0,
        }
    }

    /// Verifies that the stack canary word at the bottom of the stack has not been overwritten.
    pub fn check_stack_integrity(&self) -> bool {
        if self.stack.is_empty() {
            return true; // Main task stack is managed by bootloader
        }
        unsafe {
            let canary_ptr = self.stack.as_ptr() as *const u64;
            *canary_ptr == STACK_CANARY
        }
    }

    /// Returns the virtual top address of this task's dedicated kernel stack.
    pub fn kernel_stack_top(&self) -> u64 {
        if self.stack.is_empty() {
            0
        } else {
            (self.stack.as_ptr() as u64) + (TASK_STACK_SIZE as u64) - 16
        }
    }
}
