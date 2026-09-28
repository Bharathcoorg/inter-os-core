//! Inter OS Multitasking & Process Foundation
//!
//! Provides the core primitives for threads and processes: `TaskId`,
//! `TaskState`, `TaskControlBlock` (TCB), stack management, and scheduling.

pub mod context;
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
    USER_INIT_ELF, APP_SETTINGS_ELF, APP_TERMINAL_ELF, COMPOSITOR_ELF, LAUNCHER_ELF,
    APP_DIALER_ELF, APP_VAULT_ELF, APP_CAMERA_ELF, APP_CALCULATOR_ELF, APP_BROWSER_ELF,
    APP_MESSAGES_ELF, APP_CALENDAR_ELF, APP_MUSIC_ELF, APP_FILE_MANAGER_ELF, APP_GALLERY_ELF,
};
pub use user::{
    spawn_user_init,
    spawn_app_settings,
    spawn_app_terminal,
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
    pub stack: Box<[u8]>,
    /// Capability table holding guarded access tokens.
    pub cap_table: CapabilityTable,
    /// Total CPU timer ticks consumed by this task.
    pub cpu_ticks: u64,
    /// Priority level (higher numbers run with precedence).
    pub priority: u8,
    /// Dedicated hardware virtual memory address space (CR3 Page Table).
    pub page_table: Option<SharedAddressSpace>,
    /// Virtual address of userspace entry point (Ring 3 RIP).
    pub user_entry: usize,
    /// Virtual address of userspace stack top (Ring 3 RSP).
    pub user_stack_top: usize,
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
        let mut stack = Box::new_uninit_slice(TASK_STACK_SIZE);
        // Zero-fill stack memory
        let mut stack = unsafe {
            for byte in stack.iter_mut() {
                byte.write(0);
            }
            stack.assume_init()
        };

        // Write canary at the bottom of the stack
        unsafe {
            let canary_ptr = stack.as_mut_ptr() as *mut u64;
            *canary_ptr = STACK_CANARY;
        }

        // Initialize stack frame for context switching
        let context = unsafe { TaskContext::create_initial(&mut stack, entry_point) };

        Self {
            id: TaskId::new(),
            name,
            state: TaskState::Ready,
            context,
            stack,
            cap_table,
            cpu_ticks: 0,
            priority,
            page_table: None,
            user_entry: 0,
            user_stack_top: 0,
            process_id: None,
            parent_id: TaskId(0),
            exit_code: 0,
        }
    }

    /// Attaches an isolated hardware page table to this task.
    pub fn with_page_table(mut self, pt: crate::memory::TaskPageTable) -> Self {
        self.page_table = Some(Arc::new(Mutex::new(pt)));
        self
    }

    /// Creates an existing/current task representation (for the bootstrap main thread).
    pub fn from_current(name: &'static str, cap_table: CapabilityTable) -> Self {
        Self {
            id: TaskId(0), // Task 0 is the primordial kernel main thread
            name,
            state: TaskState::Running,
            context: TaskContext::empty(),
            stack: Box::new([]),
            cap_table,
            cpu_ticks: 0,
            priority: 0,
            page_table: None,
            user_entry: 0,
            user_stack_top: 0,
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
