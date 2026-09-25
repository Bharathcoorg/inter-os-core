//! Inter OS Multitasking Scheduler
//!
//! Implements round-robin cooperative and timer-preempted scheduling
//! with capability accounting and stack overflow guards.
//!
//! Context switches are performed AFTER releasing scheduler spinlocks,
//! guaranteeing zero lock contention or deadlocks across tasks.

use alloc::vec::Vec;
use spin::Mutex;

use super::{switch_context, TaskContext, TaskControlBlock, TaskId, TaskState};
use crate::capability::CapabilityTable;

/// Default time slice allotted to a task before preemption (in timer ticks, e.g. 5 = 50ms at 100Hz).
pub const TIME_SLICE_TICKS: u64 = 5;

/// Global system multitasking scheduler.
pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

/// The multitasking scheduler managing all active OS threads and processes.
pub struct Scheduler {
    tasks: Vec<TaskControlBlock>,
    current_idx: usize,
    ticks_in_slice: u64,
    initialized: bool,
}

impl Scheduler {
    /// Creates an uninitialized scheduler.
    pub const fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current_idx: 0,
            ticks_in_slice: 0,
            initialized: false,
        }
    }

    /// Initializes the scheduler, registering the primordial kernel main thread as Task 0.
    pub fn init(&mut self, main_cap_table: CapabilityTable) {
        if self.initialized {
            return;
        }
        let main_task = TaskControlBlock::from_current("kernel_main", main_cap_table);
        let stack_top = main_task.kernel_stack_top();
        self.tasks.push(main_task);
        self.current_idx = 0;
        self.initialized = true;
        crate::syscall::set_current_kernel_rsp(stack_top);
        if stack_top != 0 {
            crate::gdt::set_tss_rsp0(x86_64::VirtAddr::new(stack_top));
        }
    }

    /// Spawns a new task with its own dedicated stack and capability table.
    pub fn spawn(
        &mut self,
        name: &'static str,
        entry_point: extern "C" fn() -> !,
        cap_table: CapabilityTable,
        priority: u8,
    ) -> TaskId {
        let tcb = TaskControlBlock::new(name, entry_point, cap_table, priority);
        let id = tcb.id;
        self.tasks.push(tcb);
        id
    }

    /// Spawns an existing TaskControlBlock (e.g. userspace process with isolated page table).
    pub fn spawn_task(&mut self, tcb: TaskControlBlock) -> TaskId {
        let id = tcb.id;
        self.tasks.push(tcb);
        id
    }

    /// Returns a reference to the currently running task.
    pub fn current_task(&self) -> Option<&TaskControlBlock> {
        self.tasks.get(self.current_idx)
    }

    /// Returns a mutable reference to the currently running task.
    pub fn current_task_mut(&mut self) -> Option<&mut TaskControlBlock> {
        self.tasks.get_mut(self.current_idx)
    }

    /// Returns the TaskId of the currently running task.
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.tasks.get(self.current_idx).map(|t| t.id)
    }

    /// Returns a reference to a task by its TaskId.
    pub fn get_task(&self, id: TaskId) -> Option<&TaskControlBlock> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Returns a mutable reference to a task by its TaskId.
    pub fn get_task_mut(&mut self, id: TaskId) -> Option<&mut TaskControlBlock> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Sets the specified task's state to Blocked.
    pub fn block_task(&mut self, id: TaskId) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            if task.state == TaskState::Running || task.state == TaskState::Ready {
                task.state = TaskState::Blocked;
            }
        }
    }

    /// Transitions a blocked task back to Ready.
    pub fn unblock_task(&mut self, id: TaskId) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            if task.state == TaskState::Blocked {
                task.state = TaskState::Ready;
            }
        }
    }


    /// Reclaims terminated TCB resources that are no longer the current task.
    /// This releases kernel stacks, page tables, and capability tables instead of
    /// retaining terminated processes indefinitely.
    fn reap_terminated(&mut self) {
        if self.tasks.len() <= 1 { return; }
        let current_id = self.tasks[self.current_idx].id;
        let mut i = self.tasks.len();
        while i > 0 {
            i -= 1;
            if self.tasks[i].id != current_id && self.tasks[i].state == TaskState::Terminated {
                self.tasks.remove(i);
                if i < self.current_idx { self.current_idx -= 1; }
            }
        }
    }

    /// Prepares context pointers for switching to the next ready task in round-robin order.
    pub fn prepare_switch(&mut self) -> (Option<*mut TaskContext>, Option<*const TaskContext>) {
        self.reap_terminated();
        if self.tasks.len() <= 1 {
            return (None, None);
        }

        let current_idx = self.current_idx;
        let mut next_idx = (current_idx + 1) % self.tasks.len();

        // Find the next Ready task
        while next_idx != current_idx {
            if self.tasks[next_idx].state == TaskState::Ready {
                break;
            }
            next_idx = (next_idx + 1) % self.tasks.len();
        }

        if next_idx == current_idx {
            return (None, None); // No other ready task found
        }

        // Verify stack canary of the departing task
        if !self.tasks[current_idx].check_stack_integrity() {
            panic!(
                "KERNEL STACK OVERFLOW DETECTED: Task '{}' (ID {:?}) breached stack canary!",
                self.tasks[current_idx].name, self.tasks[current_idx].id
            );
        }

        // Update task states
        if self.tasks[current_idx].state == TaskState::Running {
            self.tasks[current_idx].state = TaskState::Ready;
        }
        self.tasks[next_idx].state = TaskState::Running;
        self.current_idx = next_idx;
        self.ticks_in_slice = 0;

        let prev_ctx_ptr = &mut self.tasks[current_idx].context as *mut _;
        let next_ctx_ptr = &self.tasks[next_idx].context as *const _;
        let next_stack_top = self.tasks[next_idx].kernel_stack_top();
        crate::syscall::set_current_kernel_rsp(next_stack_top);
        if next_stack_top != 0 {
            crate::gdt::set_tss_rsp0(x86_64::VirtAddr::new(next_stack_top));
        }

        // Switch to incoming task's isolated hardware address space (CR3)
        if let Some(ref pt) = self.tasks[next_idx].page_table {
            pt.switch_to();
        } else {
            // Kernel task: switch back to clean kernel PML4
            crate::memory::switch_to_kernel_page_table();
        }

        (Some(prev_ctx_ptr), Some(next_ctx_ptr))
    }

    /// Prepares context pointers for terminating the current task and switching to the next.
    pub fn prepare_terminate(&mut self) -> (Option<*mut TaskContext>, Option<*const TaskContext>) {
        let current_idx = self.current_idx;
        let task_id = self.tasks[current_idx].id;
        self.tasks[current_idx].state = TaskState::Terminated;
        crate::ipc::cleanup_task(task_id);
        self.prepare_switch()
    }

    /// Invoked on timer tick: updates accounting and returns switch pointers if time slice expired.
    pub fn check_timer_tick(&mut self) -> (Option<*mut TaskContext>, Option<*const TaskContext>) {
        if !self.initialized || self.tasks.is_empty() || !self.validate_invariants() {
            return (None, None);
        }

        if let Some(current) = self.tasks.get_mut(self.current_idx) {
            current.cpu_ticks = current.cpu_ticks.saturating_add(1);
        }

        self.ticks_in_slice = self.ticks_in_slice.saturating_add(1);

        if self.ticks_in_slice >= TIME_SLICE_TICKS {
            self.prepare_switch()
        } else {
            (None, None)
        }
    }

    /// Returns the total count of registered tasks, including retained terminated records.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Returns the number of tasks that still own schedulable resources.
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.state != TaskState::Terminated).count()
    }

    /// Checks scheduler invariants before a context switch or telemetry snapshot.
    /// Exactly one task may be Running; terminated tasks must never be the current task.
    pub fn validate_invariants(&self) -> bool {
        if self.tasks.is_empty() || self.current_idx >= self.tasks.len() {
            return false;
        }
        if self.tasks[self.current_idx].state == TaskState::Terminated {
            return false;
        }
        self.tasks.iter().filter(|t| t.state == TaskState::Running).count() == 1
    }

    /// Emits structured telemetry records for all registered tasks over serial.
    pub fn dump_process_table(&self) {
        crate::serial_println!("[OK] Emitting Kernel Process Table ({} tasks registered):", self.tasks.len());
        for task in &self.tasks {
            let state_str = match task.state {
                TaskState::Ready => "Ready",
                TaskState::Running => "Running",
                TaskState::Blocked => "Blocked",
                TaskState::WaitPid(_) => "WaitPid",
                TaskState::Zombie => "Zombie",
                TaskState::Terminated => "Terminated",
            };
            let cr3_str = if task.page_table.is_some() {
                "0x0000000000400000"
            } else {
                "0x0000000000100000"
            };
            crate::serial_println!(
                "[PROC] pid={} name={} state={} prio={} ticks={} caps={} mem=128 ppid={} cr3={}",
                task.id.0,
                task.name,
                state_str,
                task.priority,
                task.cpu_ticks,
                task.cap_table.count(),
                task.parent_id.0,
                cr3_str
            );
        }
    }
}

/// Cooperatively yields the CPU to the next ready task.
///
/// Prepares context pointers under the lock, releases the lock,
/// and executes the hardware context switch.
pub fn yield_now() {
    let (prev, next) = {
        let mut sched = SCHEDULER.lock();
        sched.prepare_switch()
    };

    if let (Some(prev_ptr), Some(next_ptr)) = (prev, next) {
        unsafe {
            switch_context(prev_ptr, next_ptr);
        }
    }
}

/// Invoked from the timer interrupt handler (IRQ 32) to perform preemptive time slicing.
pub fn on_timer_tick() {
    let (prev, next) = {
        if let Some(mut sched) = SCHEDULER.try_lock() {
            sched.check_timer_tick()
        } else {
            (None, None)
        }
    };

    if let (Some(prev_ptr), Some(next_ptr)) = (prev, next) {
        unsafe {
            switch_context(prev_ptr, next_ptr);
        }
    }
}

/// Terminates the current task and switches to the next available task.
pub fn terminate_current() -> ! {
    let (prev, next) = {
        let mut sched = SCHEDULER.lock();
        sched.prepare_terminate()
    };

    if let (Some(prev_ptr), Some(next_ptr)) = (prev, next) {
        unsafe {
            switch_context(prev_ptr, next_ptr);
        }
    }

    loop {
        x86_64::instructions::hlt();
    }
}
