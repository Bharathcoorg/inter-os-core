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
        self.spawn_with_parent(name, entry_point, cap_table, priority, TaskId(0))
    }

    /// Spawns a task with explicit parent ownership. Userspace services are
    /// children of PID 1 so their lifecycle can be observed and reaped there.
    pub fn spawn_with_parent(
        &mut self,
        name: &'static str,
        entry_point: extern "C" fn() -> !,
        cap_table: CapabilityTable,
        priority: u8,
        parent_id: TaskId,
    ) -> TaskId {
        let mut tcb = TaskControlBlock::new(name, entry_point, cap_table, priority);
        tcb.parent_id = parent_id;
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
        if self.current_idx >= self.tasks.len() {
            self.current_idx = self.tasks.len().saturating_sub(1);
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
        let mut best_idx = None;
        let mut best_priority = 0u8;
        while next_idx != current_idx {
            if self.tasks[next_idx].state == TaskState::Ready {
                let p = self.tasks[next_idx].priority;
                if best_idx.is_none() || p > best_priority {
                    best_idx = Some(next_idx);
                    best_priority = p;
                }
            }
            next_idx = (next_idx + 1) % self.tasks.len();
        }
        let Some(next_idx) = best_idx else {
            return (None, None);
        };

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

        // An exiting task remains a Zombie until its parent reaps it. Terminated
        // is reserved for the post-waitpid state so resources are not reclaimed
        // before the parent observes the exit status.
        self.tasks[current_idx].state = TaskState::Zombie;
        crate::ipc::cleanup_task(task_id);

        // Reparent live children to primordial init so a dead parent cannot leave
        // an unreapable child/zombie subtree behind.
        for task in &mut self.tasks {
            if task.id != task_id && task.parent_id == task_id {
                task.parent_id = TaskId(0);
            }
        }

        self.prepare_switch()
    }

    /// Invoked on timer tick: updates accounting and returns switch pointers if time slice expired.
    pub fn check_timer_tick(&mut self) -> (Option<*mut TaskContext>, Option<*const TaskContext>) {
        if !self.initialized || self.tasks.is_empty() {
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

    /// Reaps a terminated child owned by parent_id, returning its exit code.
    ///
    /// Reaping is only allowed for Zombie/Terminated children and never removes
    /// the currently running task. Dropping the TCB releases its kernel stack,
    /// page table and capability table.
    pub fn terminate_task(&mut self, id: TaskId, exit_code: i32) -> Result<(), &'static str> {
        let current_id = self.current_task_id().ok_or("no current task")?;
        if id == TaskId(0) || id == current_id {
            return Err("task cannot be terminated by lifecycle manager");
        }
        let task = self.tasks.iter_mut().find(|task| task.id == id).ok_or("task not found")?;
        if matches!(task.state, TaskState::Terminated | TaskState::Zombie) {
            return Ok(());
        }
        task.exit_code = exit_code;
        task.state = TaskState::Zombie;
        crate::ipc::cleanup_task(id);
        crate::drivers::display_kms::DRM_KMS.lock().destroy_surfaces_owned_by(id.0);
        // Do not update the service registry while holding the scheduler lock.
        // The registry's supervision path deliberately acquires scheduler -> registry;
        // taking the inverse order here would deadlock recovery.
        Ok(())
    }

    pub fn reap_child(&mut self, parent_id: TaskId, child_id: TaskId) -> Result<i32, &'static str> {
        if child_id == TaskId(0) || child_id == self.tasks.get(self.current_idx).map(|t| t.id).unwrap_or(TaskId(0)) {
            return Err("child cannot be reaped");
        }
        let index = self.tasks.iter().position(|task| {
            task.id == child_id
                && task.parent_id == parent_id
                && matches!(task.state, TaskState::Zombie | TaskState::Terminated)
        }).ok_or("child not ready or not owned by caller")?;
        let exit_code = self.tasks[index].exit_code;
        self.tasks.remove(index);
        if index < self.current_idx {
            self.current_idx -= 1;
        }
        if self.current_idx >= self.tasks.len() && !self.tasks.is_empty() {
            self.current_idx = self.tasks.len() - 1;
        }
        Ok(exit_code)
    }

    /// Returns the total count of registered tasks.
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Returns the number of tasks that still own schedulable resources.
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.state != TaskState::Terminated).count()
    }

    /// Checks scheduler invariants before a context switch or telemetry snapshot.
    pub fn validate_invariants(&self) -> bool {
        if self.tasks.is_empty() || self.current_idx >= self.tasks.len() { return false; }
        if self.tasks[self.current_idx].state == TaskState::Terminated { return false; }
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
                "[PROC] pid={} name={} state={} prio={} ticks={} caps={} kstack={} ppid={} cr3={}",
                task.id.0,
                task.name,
                state_str,
                task.priority,
                task.cpu_ticks,
                task.cap_table.count(),
                super::TASK_STACK_SIZE / 1024,
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

    // Scheduler lock is released before service supervision.
    crate::services::supervise();

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
    let (exiting_id, exit_code, prev, next) = {
        let mut sched = SCHEDULER.lock();
        let id = sched.current_task_id().unwrap_or(TaskId(0));
        let code = sched.current_task().map(|t| t.exit_code).unwrap_or(0);
        let (prev, next) = sched.prepare_terminate();
        (id, code, prev, next)
    };

    // Publish the exit only after releasing the scheduler lock. Service
    // supervision can then safely acquire its registry without lock inversion.
    crate::services::record_exit(exiting_id, exit_code);

    if let (Some(prev_ptr), Some(next_ptr)) = (prev, next) {
        unsafe {
            switch_context(prev_ptr, next_ptr);
        }
    }

    loop {
        x86_64::instructions::hlt();
    }
}
