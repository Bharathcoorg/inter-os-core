// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Inter OS Multitasking Scheduler
//!
//! Implements round-robin cooperative and timer-preempted scheduling
//! with capability accounting and stack overflow guards.
//!
//! Context switches are performed AFTER releasing scheduler spinlocks,
//! guaranteeing zero lock contention or deadlocks across tasks.

use alloc::vec::Vec;
use crate::sync::Mutex;

use super::{switch_context, TaskContext, TaskControlBlock, TaskId, TaskState};
use crate::capability::CapabilityTable;
use crate::task::process::ReapError;

/// Default time slice allotted to a task before preemption.
///
/// The unit is a real millisecond, because `SYSTEM_TICKS` is the HPET-derived
/// count and the slice is now aged by the same elapsed measure. It used to be
/// counted in timer *deliveries*, which is a host decision: a host that
/// resolves its timers at 15.6 ms made five of them ~78 ms, so a task woken by
/// a touch waited that long for the CPU while the device still called its clock
/// milliseconds. See `check_timer_tick`.
pub const TIME_SLICE_TICKS: u64 = 5;

/// Standard priority levels for system and user workloads.
pub const PRIORITY_IDLE: u8 = 0;
pub const PRIORITY_BACKGROUND: u8 = 2;
pub const PRIORITY_SERVICE: u8 = 5;
pub const PRIORITY_APP: u8 = 6;
pub const PRIORITY_REALTIME: u8 = 10;

/// Highest task id the boot services occupy. `configure_task_ui_pipeline` pins ids
/// 2, 3 and 10 as realtime UI tasks, so a task at or below this ceiling is never
/// demoted when the foreground surface moves away -- only real apps are.
pub const BOOT_SERVICE_ID_CEILING: u64 = 10;

/// Strict VSync frame deadline budgets.
/// For 120Hz display modes: <8.3ms (8,333,333 ns).
pub const VSYNC_BUDGET_120HZ_NS: u64 = 8_333_333;
/// For 60Hz display modes: <16.6ms (16,666,666 ns).
pub const VSYNC_BUDGET_60HZ_NS: u64 = 16_666_666;

/// Global system multitasking scheduler.
pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

/// One departing task whose cross-subsystem state still needs reclaiming.
#[derive(Clone, Copy)]
struct DeferredCleanup {
    id: TaskId,
    name: &'static str,
}

/// Bound on terminations awaiting reclaim. The kernel main loop drains the
/// queue on every pass, so it only has to cover exits inside a single pass.
const DEFERRED_CLEANUP_SLOTS: usize = 64;
static DEFERRED_CLEANUP: Mutex<[Option<DeferredCleanup>; DEFERRED_CLEANUP_SLOTS]> =
    Mutex::new([None; DEFERRED_CLEANUP_SLOTS]);

/// Queues cross-subsystem reclaim for a task that has left the run queue.
///
/// WHY the hooks are not run at the termination site: both termination paths
/// execute with `SCHEDULER` held, and every hook acquires another subsystem's
/// lock; `fs::cleanup_task` takes `VFS`. `vfs::app_package_for_task` runs the
/// opposite order (VFS, then `SCHEDULER`). With interrupts enabled a waiter
/// inside a syscall still cannot be preempted once it owns the scheduler, so
/// the parallel task holding VFS never runs again and both spin forever. The
/// task is only marked here; `run_deferred_cleanup` performs the reclaim in a
/// context that owns no scheduler lock.
pub fn defer_cleanup(id: TaskId, name: &'static str) {
    let mut pending = DEFERRED_CLEANUP.lock();
    if pending.iter().any(|slot| matches!(slot, Some(entry) if entry.id == id)) {
        return;
    }
    match pending.iter_mut().find(|slot| slot.is_none()) {
        Some(slot) => *slot = Some(DeferredCleanup { id, name }),
        None => crate::serial_println!(
            "[LIFECYCLE] deferred cleanup queue full; task {} resources not reclaimed",
            id.0
        ),
    }
}

/// Reclaims resources of tasks that terminated since the last pass.
///
/// MUST be called without `SCHEDULER` held. The hooks below may block on the
/// subsystem they own; that is safe exactly here, because the caller owns
/// nothing the scheduler needs and the timer can preempt it.
pub fn run_deferred_cleanup() {
    loop {
        let entry = {
            let mut pending = DEFERRED_CLEANUP.lock();
            match pending.iter_mut().find(|slot| slot.is_some()) {
                Some(slot) => slot.take(),
                None => return,
            }
        };
        let Some(entry) = entry else { return; };
        reclaim_task(entry);
    }
}

/// Releases everything a departed task owned outside the scheduler.
fn reclaim_task(entry: DeferredCleanup) {
    crate::ipc::cleanup_task(entry.id);
    crate::fs::cleanup_task(entry.id);
    crate::event::cleanup_task(entry.id);
    crate::net::cleanup_task(entry.id);
    // Authority follows task lifetime. `capability::cleanup_task` revokes the
    // exiting task's delegated leases, prunes them from the kernel-wide
    // registry and drops its consent records; without this a dead task kept
    // live delegated authority and `has_permission` kept answering for it.
    crate::capability::cleanup_task(entry.id);

    // Surface ownership follows task lifetime. Reclaim all app surfaces at
    // the common exit path so normal std::exit() cannot leave stale windows.
    //
    // WHY the shell is raised at all: an application is a layer over the
    // desktop. When the layer is gone and the desktop stays hidden, the
    // screen composites the system band over an empty canvas, which is the
    // blank white screen a person sees after pressing back.
    let shell_owner = SCHEDULER
        .lock()
        .tasks
        .iter()
        .find(|t| t.name == "launcher")
        .map(|t| t.id.0);
    {
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        kms.destroy_surfaces_owned_by(entry.id.0);
        // WHY not gated on a reclaimed surface: the task may have taken its
        // own surface down before it died, and the screen still has to come
        // back to the desktop. See the exit syscall for the same rule.
        if let Some(shell) = shell_owner {
            if shell != entry.id.0 {
                kms.set_surfaces_visibility_for_task(shell, true);
            }
        }
    }

    let user_id = crate::task::process::user_for_task(entry.id).unwrap_or(0);
    crate::notification::NOTIFICATION_MANAGER
        .lock()
        .cancel_all_for_package(entry.name, user_id);
}

/// The multitasking scheduler managing all active OS threads and processes.
pub struct Scheduler {
    tasks: Vec<TaskControlBlock>,
    current_idx: usize,
    ticks_in_slice: u64,
    initialized: bool,
    active_foreground_task: Option<TaskId>,
}

/// Whether a terminated task's process and thread records are now releasable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReapOutcome {
    /// The records are gone (or were never there). The task table entry is
    /// stale and the task control block may be dropped.
    Released,
    /// A required table was still held after a bounded retry. The task table
    /// entry is kept and retried on a later tick.
    Deferred,
}

/// Bounded spin budget for a reap that lost a race on the process or thread
/// table. This is a fixed count of `spin_loop()` hints, never a blocking lock
/// and never an unbounded wait, so it stays safe in interrupt context.
const REAP_SPIN_BUDGET: usize = 100;

/// Attempts to release a terminated task's records, retrying a fixed number of
/// times if the process or thread table is momentarily held elsewhere.
///
/// WHY bounded and then deferred rather than retried forever: on this
/// uniprocessor port the holder may be a task that is not going to run until
/// this one yields, so spinning past this budget only delays the tick that
/// would let it finish. Deferring keeps the reaping attempt in the tick loop
/// where it is observable and bounded.
///
/// WHY the match is exhaustive over [`ReapError`]: the only safe action for
/// `TablesBusy` is to keep the task table entry. An unhandled arm here would
/// have to guess, and guessing "release it" leaks the process and thread table
/// entries -- the silent zombie leak this path exists to prevent.
fn reap_task_now_or_defer(task: TaskId) -> ReapOutcome {
    let mut outcome = match crate::task::process::try_reap(task) {
        Ok(()) => return ReapOutcome::Released,
        Err(ReapError::ThreadNotRegistered) | Err(ReapError::ProcessRecordMissing) => ReapOutcome::Released,
        Err(ReapError::TablesBusy) => ReapOutcome::Deferred,
    };
    for _ in 0..REAP_SPIN_BUDGET {
        core::hint::spin_loop();
        outcome = match crate::task::process::try_reap(task) {
            Ok(()) => ReapOutcome::Released,
            Err(ReapError::ThreadNotRegistered) | Err(ReapError::ProcessRecordMissing) => ReapOutcome::Released,
            Err(ReapError::TablesBusy) => ReapOutcome::Deferred,
        };
        if outcome == ReapOutcome::Released {
            break;
        }
    }
    outcome
}

impl Scheduler {
    /// Creates an uninitialized scheduler.
    pub const fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current_idx: 0,
            ticks_in_slice: 0,
            initialized: false,
            active_foreground_task: None,
        }
    }

    /// Automatically configures UI pipeline services to Real-Time EDF scheduling.
    fn configure_task_ui_pipeline(&mut self, id: TaskId) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            if task.name == "displayd"
                || task.name == "inputd"
                || task.name == "compositor"
                || task.id == TaskId(2)
                || task.id == TaskId(3)
                || task.id == TaskId(10)
            {
                task.sched_class = super::SchedulingClass::RealTime;
                task.priority = PRIORITY_REALTIME;
                task.frame_budget_ns = VSYNC_BUDGET_60HZ_NS;
            }
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
        let Some(process_id) = crate::task::process::register_leader(id, parent_id) else {
            return TaskId(0);
        };
        tcb.process_id = Some(process_id);
        self.tasks.push(tcb);
        self.configure_task_ui_pipeline(id);
        id
    }

    /// Spawns an existing TaskControlBlock (e.g. userspace process with isolated page table).
    pub fn spawn_task(&mut self, mut tcb: TaskControlBlock) -> TaskId {
        let id = tcb.id;
        tcb.cap_table.set_owner_task(id);
        if id == TaskId(0) {
            self.tasks.push(tcb);
            return id;
        }
        let parent_id = tcb.parent_id;
        let process_id = match tcb.process_id {
            Some(pid) => pid,
            None => match crate::task::process::register_leader(id, parent_id) {
                Some(pid) => pid,
                None => return TaskId(0),
            },
        };
        tcb.process_id = Some(process_id);
        self.tasks.push(tcb);
        self.configure_task_ui_pipeline(id);
        crate::ipc::IPC_ROUTER.lock().register_task(id);
        id
    }

    /// Creates a userspace thread in an existing process and shared address space with TLS and custom stack.
    pub fn spawn_user_thread_with_tls(
        &mut self,
        process_id: crate::task::ProcessId,
        user_entry: usize,
        user_stack: usize,
        tls_base: u64,
        user_arg: u64,
        cap_table: CapabilityTable,
        priority: u8,
    ) -> Result<TaskId, &'static str> {
        crate::task::process::reserve_thread(process_id)?;
        let shared_space = match self.tasks.iter()
            .find(|t| t.process_id == Some(process_id))
            .and_then(|t| t.page_table.clone()) {
            Some(space) => space,
            None => {
                crate::task::process::cancel_thread_reservation(process_id);
                return Err("process has no userspace address space");
            }
        };

        let mut tcb = TaskControlBlock::new("user_thread", crate::task::user::user_thread_entry, cap_table, priority);
        let id = tcb.id;
        let custom_stack = user_stack != 0;

        let (user_stack_top, cr3_frame) = if custom_stack {
            let pt = shared_space.lock();
            (user_stack, pt.frame())
        } else {
            if let Err(e) = crate::task::process::reserve_address_space(process_id, crate::task::user::USER_STACK_SIZE) {
                crate::task::process::cancel_thread_reservation(process_id);
                return Err(e);
            }
            let mut pt = shared_space.lock();
            let frame = pt.frame();
            match crate::task::user::map_user_thread_stack(&mut pt, id) {
                Ok(top) => (top, frame),
                Err(e) => {
                    crate::task::process::cancel_thread_reservation(process_id);
                    crate::task::process::release_address_space(process_id, crate::task::user::USER_STACK_SIZE);
                    return Err(e);
                }
            }
        };

        tcb.process_id = Some(process_id);
        tcb.user_entry = user_entry;
        tcb.user_stack_top = user_stack_top;
        tcb.tls_base = tls_base;
        tcb.user_arg = user_arg;
        tcb.cr3_frame = Some(cr3_frame);
        tcb.page_table = Some(shared_space.clone());

        if let Err(e) = crate::task::process::bind_reserved_thread(process_id, id) {
            let rollback = if !custom_stack {
                let mut pt = shared_space.lock();
                let res = crate::task::user::unmap_user_thread_stack(&mut pt, id);
                crate::task::process::release_address_space(process_id, crate::task::user::USER_STACK_SIZE);
                res
            } else {
                Ok(())
            };
            crate::task::process::cancel_thread_reservation(process_id);
            if rollback.is_err() {
                return Err("thread stack rollback failed");
            }
            return Err(e);
        }

        self.tasks.push(tcb);
        self.configure_task_ui_pipeline(id);
        let ipc_registered = crate::ipc::IPC_ROUTER.lock().register_task(id);
        if !ipc_registered {
            self.tasks.pop();
            if !custom_stack {
                let mut pt = shared_space.lock();
                let _ = crate::task::user::unmap_user_thread_stack(&mut pt, id);
                crate::task::process::release_address_space(process_id, crate::task::user::USER_STACK_SIZE);
            }
            crate::task::process::cancel_thread_reservation(process_id);
            return Err("IPC endpoint quota exceeded");
        }
        Ok(id)
    }

    /// Creates a userspace thread in an existing process and shared address space.
    pub fn spawn_user_thread(
        &mut self,
        process_id: crate::task::ProcessId,
        user_entry: usize,
        cap_table: CapabilityTable,
        priority: u8,
    ) -> Result<TaskId, &'static str> {
        self.spawn_user_thread_with_tls(process_id, user_entry, 0, 0, 0, cap_table, priority)
    }

    /// Returns a reference to the currently running task.
    pub fn current_task(&self) -> Option<&TaskControlBlock> {
        self.tasks.get(self.current_idx)
    }

    /// Returns a mutable reference to the currently running task.
    pub fn tasks_iter(&self) -> core::slice::Iter<'_, TaskControlBlock> {
        self.tasks.iter()
    }

    pub fn current_task_mut(&mut self) -> Option<&mut TaskControlBlock> {
        self.tasks.get_mut(self.current_idx)
    }

    /// Returns the TaskId of the currently running task.
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.tasks.get(self.current_idx).map(|t| t.id)
    }

    pub fn find_child_by_name(&self, parent_id: TaskId, name: &str) -> Option<TaskId> {
        self.tasks.iter()
            .find(|task| task.parent_id == parent_id && task.name == name
                && !matches!(task.state, TaskState::Zombie | TaskState::Terminated))
            .map(|task| task.id)
    }

    pub fn find_live_task_named(&self, name: &str) -> Option<TaskId> {
        self.tasks.iter()
            .find(|task| task.name == name
                && !matches!(task.state, TaskState::Zombie | TaskState::Terminated))
            .map(|task| task.id)
    }

    pub fn has_live_task_named(&self, name: &str) -> bool {
        self.find_live_task_named(name).is_some()
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

    /// Dynamically sets the specified task's scheduling priority.
    pub fn set_task_priority(&mut self, id: TaskId, priority: u8) {
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
            task.priority = priority;
        }
    }

    /// Returns true if `task` belongs to the deterministic Real-Time UI/VSync pipeline.
    pub fn is_realtime_ui_task(&self, task: &TaskControlBlock) -> bool {
        task.sched_class == super::SchedulingClass::RealTime
            || task.name == "displayd"
            || task.name == "inputd"
            || task.name == "compositor"
            || task.id == TaskId(2)
            || task.id == TaskId(3)
            || task.id == TaskId(10)
            || Some(task.id) == self.active_foreground_task
    }

    /// Dynamically assigns the active foreground surface task for real-time UI scheduling.
    pub fn set_active_foreground_task(&mut self, task: Option<TaskId>) {
        let prev = self.active_foreground_task;
        self.active_foreground_task = task;
        if let Some(prev_id) = prev {
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == prev_id) {
                if t.name != "displayd" && t.name != "inputd" && t.name != "compositor" && t.id.0 > BOOT_SERVICE_ID_CEILING {
                    t.priority = PRIORITY_APP;
                    t.sched_class = super::SchedulingClass::Normal;
                    t.deadline_ticks = 0;
                }
            }
        }
        if let Some(next_id) = task {
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == next_id) {
                t.priority = PRIORITY_REALTIME;
                t.sched_class = super::SchedulingClass::RealTime;
                t.frame_budget_ns = VSYNC_BUDGET_60HZ_NS;
            }
        }
    }

    /// Returns the currently designated active foreground surface task.
    pub fn active_foreground_task(&self) -> Option<TaskId> {
        self.active_foreground_task
    }

    /// Sets a real-time task's frame deadline and budget for Earliest-Deadline-First dispatch.
    pub fn set_task_deadline(&mut self, id: TaskId, deadline_ticks: u64, budget_ns: u64) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.sched_class = super::SchedulingClass::RealTime;
            t.priority = t.priority.max(PRIORITY_REALTIME);
            t.deadline_ticks = deadline_ticks;
            t.frame_budget_ns = budget_ns;
        }
    }

    /// Sets the task's scheduling class.
    pub fn set_task_sched_class(&mut self, id: TaskId, class: super::SchedulingClass) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.sched_class = class;
            if class == super::SchedulingClass::RealTime {
                t.priority = t.priority.max(PRIORITY_REALTIME);
            }
        }
    }


    /// Reclaims terminated TCB resources that are no longer the current task.
    fn reap_terminated(&mut self) {
        if self.tasks.len() <= 1 { return; }
        let current_id = self.tasks.get(self.current_idx).map(|t| t.id).unwrap_or(TaskId(0));
        let mut i = self.tasks.len();
        while i > 0 {
            i -= 1;
            if self.tasks[i].id != current_id && self.tasks[i].state == TaskState::Terminated {
                let tid = self.tasks[i].id;
                match reap_task_now_or_defer(tid) {
                    ReapOutcome::Released => {
                        self.tasks.remove(i);
                        if i < self.current_idx { self.current_idx -= 1; }
                    }
                    ReapOutcome::Deferred => { /* retried on a later tick */ }
                }
            }
        }
        if self.current_idx >= self.tasks.len() && !self.tasks.is_empty() {
            self.current_idx = self.tasks.len() - 1;
        }
    }

    /// Reclaims orphaned zombies whose parent is TaskId(0) and are not the current task.
    ///
    /// When a parent process terminates before its children, live children are reparented
    /// to TaskId(0). When those orphaned children (or development-bridge tasks launched
    /// directly under TaskId(0)) exit, they transition to Zombie. Because TaskId(0) is the
    /// primordial kernel task and never executes sys_waitpid, the kernel must reclaim
    /// their process records, kernel stacks, page tables, and capability tables.
    pub fn reap_orphans(&mut self) -> usize {
        if self.tasks.len() <= 1 { return 0; }
        let current_id = self.tasks.get(self.current_idx).map(|t| t.id).unwrap_or(TaskId(0));
        let mut reaped = 0;
        let mut i = self.tasks.len();
        while i > 0 {
            i -= 1;
            let task = &self.tasks[i];
            if task.id != TaskId(0)
                && task.id != current_id
                && task.parent_id == TaskId(0)
                && matches!(task.state, TaskState::Zombie | TaskState::Terminated)
            {
                let tid = task.id;
                match reap_task_now_or_defer(tid) {
                    ReapOutcome::Released => {
                        self.tasks.remove(i);
                        reaped += 1;
                        if i < self.current_idx {
                            self.current_idx -= 1;
                        }
                    }
                    ReapOutcome::Deferred => { /* retried on a later tick */ }
                }
            }
        }
        if self.current_idx >= self.tasks.len() && !self.tasks.is_empty() {
            self.current_idx = self.tasks.len() - 1;
        }
        reaped
    }

    /// Prepares context pointers for switching to the next ready task in round-robin order.
    pub fn prepare_switch(&mut self) -> (Option<*mut TaskContext>, Option<*const TaskContext>) {
        self.reap_terminated();
        self.reap_orphans();
        if self.tasks.len() <= 1 {
            return (None, None);
        }

        let current_idx = self.current_idx;

        // 1. Scan for Ready tasks in the Real-Time / EDF class
        let mut best_rt_idx = None;
        let mut earliest_deadline = u64::MAX;
        let mut best_rt_priority = 0u8;

        for (i, task) in self.tasks.iter().enumerate() {
            if i != current_idx && task.state == TaskState::Ready && self.is_realtime_ui_task(task) {
                let dl = if task.deadline_ticks > 0 { task.deadline_ticks } else { u64::MAX - 1 };
                if best_rt_idx.is_none()
                    || dl < earliest_deadline
                    || (dl == earliest_deadline && task.priority > best_rt_priority)
                {
                    best_rt_idx = Some(i);
                    earliest_deadline = dl;
                    best_rt_priority = task.priority;
                }
            }
        }

        let next_idx = if let Some(rt_idx) = best_rt_idx {
            rt_idx
        } else {
            // 2. Normal round-robin with priority selection
            let mut scan_idx = (current_idx + 1) % self.tasks.len();
            let mut best_idx = None;
            let mut best_priority = 0u8;
            while scan_idx != current_idx {
                if self.tasks[scan_idx].state == TaskState::Ready {
                    let p = self.tasks[scan_idx].priority;
                    if best_idx.is_none() || p > best_priority {
                        best_idx = Some(scan_idx);
                        best_priority = p;
                    }
                }
                scan_idx = (scan_idx + 1) % self.tasks.len();
            }
            let Some(idx) = best_idx else {
                return (None, None);
            };
            idx
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

        // Switch to incoming task's isolated hardware address space (CR3) without locking
        if let Some(frame) = self.tasks[next_idx].cr3_frame {
            crate::memory::switch_to_task_page_table(frame);
        } else {
            // Kernel task: switch back to clean kernel PML4
            crate::memory::switch_to_kernel_page_table();
        }

        // Restore Thread-Local Storage (TLS) FS base register for incoming task
        unsafe {
            crate::syscall::entry::wrmsr(
                crate::syscall::entry::msr::IA32_FS_BASE,
                self.tasks[next_idx].tls_base,
            );
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
        // A non-leader userspace thread owns only its dedicated guarded stack.
        // Reclaim that mapping before the TCB can be reaped, while the shared
        // process address space remains alive for sibling threads.
        if task_id != TaskId(0) {
            if let Some(process_id) = self.tasks[current_idx].process_id {
                if !crate::task::process::is_leader(task_id) {
                    if let Some(ref shared_space) = self.tasks[current_idx].page_table {
                        let mut pt = shared_space.lock();
                        let _ = crate::task::user::unmap_user_thread_stack(&mut pt, task_id);
                    }
                    crate::task::process::release_address_space(
                        process_id,
                        crate::task::user::USER_STACK_SIZE,
                    );
                }
            }
        }

        self.tasks[current_idx].state = TaskState::Zombie;
        if task_id != TaskId(0) {
            crate::task::process::mark_zombie(task_id, self.tasks[current_idx].exit_code);
        }
        defer_cleanup(task_id, self.tasks[current_idx].name);

        // Audit capability telemetry before table revocation
        let total_cap_invocations: u64 = (0..self.tasks[current_idx].cap_table.capacity())
            .filter_map(|slot| self.tasks[current_idx].cap_table.access_stats(slot))
            .map(|(count, _)| count)
            .sum();
        if total_cap_invocations > 0 {
            crate::serial_println!(
                "[CAP AUDIT] task={} ({}) completed with {} capability invocations",
                task_id.0,
                self.tasks[current_idx].name,
                total_cap_invocations
            );
        }

        self.tasks[current_idx].cap_table.revoke_all();

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
    ///
    /// `elapsed_ms` is what this one delivery of the timer stood for -- the same
    /// measure `SYSTEM_TICKS` was advanced by. The slice has to be aged by it
    /// rather than by one per delivery, or the slice a task gets before its
    /// blocked neighbour is scheduled is set by the host's timer resolution
    /// instead of by the device.
    pub fn check_timer_tick(
        &mut self,
        elapsed_ms: u64,
    ) -> (Option<*mut TaskContext>, Option<*const TaskContext>) {
        if !self.initialized || self.tasks.is_empty() {
            return (None, None);
        }

        if let Some(current) = self.tasks.get_mut(self.current_idx) {
            current.cpu_ticks = current.cpu_ticks.saturating_add(1);
            crate::task::process::account_background_cpu_tick(current.id);
            // Real-time task frame deadline check:
            if current.sched_class == super::SchedulingClass::RealTime && current.deadline_ticks > 0 {
                let now = crate::interrupts::get_system_ticks();
                if now > current.deadline_ticks {
                    current.deadline_ticks = 0;
                }
            }
        }

        self.ticks_in_slice = self.ticks_in_slice.saturating_add(elapsed_ms.max(1));

        // Real-time UI tasks get rapid low-latency 1-tick slices when other RT tasks wait
        let has_competing_rt = self.tasks.iter().enumerate().any(|(i, t)| {
            i != self.current_idx && t.state == TaskState::Ready && self.is_realtime_ui_task(t)
        });

        let slice_limit = if has_competing_rt { 1 } else { TIME_SLICE_TICKS };

        // WHY preemption is deferred here: a task can hold a kernel spinlock
        // (VFS, process table, ...) when the tick lands. Switching it out now
        // and then marking it Blocked — which the budget path below does —
        // would strand that lock on a task that can never run again, on a
        // single CPU with no other core to release it. The lock wrappers in
        // `crate::sync` raise this gate for their critical sections; the tick
        // keeps accounting, waking and rescheduling once the gate drops.
        if self.ticks_in_slice >= slice_limit && !crate::sync::preemption_disabled() {
            let suspended = crate::task::process::enforce_background_budget();
            if suspended > 0 {
                crate::serial_println!("[LIFECYCLE] Suspended {} background application(s) after budget exhaustion.", suspended);
            }
            if let Some(current) = self.tasks.get(self.current_idx) {
                let blocked = current.process_id
                    .and_then(crate::task::process::record_for_process)
                    .map(|r| r.state == crate::task::process::ProcessState::Suspended)
                    .unwrap_or(false);
                if blocked {
                    let current_id = current.id;
                    if let Some(task) = self.tasks.iter_mut().find(|t| t.id == current_id) {
                        task.state = TaskState::Blocked;
                    }
                }
            }
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
        if let Some(process_id) = task.process_id {
            if !crate::task::process::is_leader(id) {
                if let Some(ref shared_space) = task.page_table {
                    let mut pt = shared_space.lock();
                    let _ = crate::task::user::unmap_user_thread_stack(&mut pt, id);
                }
                crate::task::process::release_address_space(
                    process_id,
                    crate::task::user::USER_STACK_SIZE,
                );
            }
        }
        let task_name = task.name;
        task.exit_code = exit_code;
        task.state = TaskState::Zombie;
        crate::task::process::mark_zombie(id, exit_code);
        defer_cleanup(id, task_name);

        // Reparent live children to primordial init so a dead parent cannot leave
        // an unreapable child/zombie subtree behind.
        for t in &mut self.tasks {
            if t.id != id && t.parent_id == id {
                t.parent_id = TaskId(0);
            }
        }
        // Do not update the service registry while holding the scheduler lock.
        // The registry's supervision path deliberately acquires scheduler -> registry;
        // taking the inverse order here would deadlock recovery.
        Ok(())
    }

    pub fn terminate_child(&mut self, parent_id: TaskId, child_id: TaskId, exit_code: i32) -> Result<(), &'static str> {
        if child_id == TaskId(0) || child_id == parent_id {
            return Err("invalid child task");
        }
        let task = self.tasks.iter().find(|task| task.id == child_id).ok_or("task not found")?;
        if task.parent_id != parent_id {
            return Err("task is not owned by caller");
        }
        self.terminate_task(child_id, exit_code)
    }

    /// Reaps a terminated child owned by `parent_id`, returning its exit code.
    ///
    /// Success means both sides are gone: the task control block and whatever the
    /// process tables still held for that task -- its thread binding, and its
    /// process record when no sibling thread kept it alive. A caller that gets
    /// `Err` back while the child is still a zombie saw the process side refuse to
    /// commit and may retry.
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
        // WHY the task control block is dropped only after this match commits:
        // `reap` gives up with `TablesBusy` once its bounded spin is spent, and
        // removing the block before the process side releases its record leaves a
        // `PROCESS_TABLE` entry owned by nobody. Task ids come from one monotone
        // counter, so the next spawn handed this id inherits that record. The
        // match is exhaustive over [`ReapError`] for the same reason as
        // `reap_task_now_or_defer`: a guessed arm would have to choose between
        // this leak and an unreapable zombie. A deferred child stays a zombie and
        // stays owned by its parent, so the parent's next `waitpid` retries it.
        match crate::task::process::reap(child_id) {
            Ok(()) => {}
            Err(ReapError::ThreadNotRegistered) | Err(ReapError::ProcessRecordMissing) => {}
            Err(ReapError::TablesBusy) => return Err("child process record still held"),
        }
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

    /// Returns the number of tasks that can still participate in scheduling.
    /// Zombies remain registered for parent reaping but are no longer schedulable.
    pub fn active_task_count(&self) -> usize {
        self.tasks.iter().filter(|t| {
            matches!(t.state, TaskState::Ready | TaskState::Running | TaskState::Blocked | TaskState::WaitPid(_))
        }).count()
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
            let cr3 = task.cr3_frame
                .map(|f| f.start_address().as_u64())
                .or_else(|| task.page_table.as_ref().map(|pt| pt.lock().frame().start_address().as_u64()))
                .unwrap_or_else(|| crate::memory::kernel_pml4().start_address().as_u64());
            crate::serial_println!(
                "[PROC] pid={} name={} state={} prio={} ticks={} caps={} kstack={} ppid={} cr3={:#x}",
                task.id.0,
                task.name,
                state_str,
                task.priority,
                task.cpu_ticks,
                task.cap_table.count(),
                super::TASK_STACK_SIZE / 1024,
                task.parent_id.0,
                cr3
            );
        }
    }
}

/// Cooperatively yields the CPU to the next ready task.
///
/// Prepares context pointers under the lock, releases the lock,
/// and executes the hardware context switch atomically with interrupts disabled.
pub fn yield_now() -> bool {
    // A cooperative switch while a lock guard is alive would hand the CPU to a
    // task that then blocks on that same lock. Refuse instead of stranding it.
    if crate::sync::preemption_disabled() {
        return false;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let (prev, next) = {
            let mut sched = SCHEDULER.lock();
            sched.prepare_switch()
        };

        if let (Some(prev_ptr), Some(next_ptr)) = (prev, next) {
            unsafe {
                switch_context(prev_ptr, next_ptr);
            }
            true
        } else {
            false
        }
    })
}

/// Invoked from the timer interrupt handler (IRQ 32) to perform preemptive time slicing.
pub fn on_timer_tick(elapsed_ms: u64) {
    let ticks = crate::interrupts::get_system_ticks();
    if let Some(mut fm) = crate::task::futex::FUTEX_MANAGER.try_lock() {
        let timed_out = fm.check_timeouts(ticks);
        if !timed_out.is_empty() {
            if let Some(mut sched) = SCHEDULER.try_lock() {
                for tid in timed_out {
                    sched.unblock_task(tid);
                }
            }
        }
    }

    let (prev, next) = {
        if let Some(mut sched) = SCHEDULER.try_lock() {
            sched.check_timer_tick(elapsed_ms)
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
    // This task still owns no scheduler lock, so it can advance reclaims queued
    // by earlier exits while it is being torn down.
    run_deferred_cleanup();

    let (exiting_id, exit_code) = {
        let sched = SCHEDULER.lock();
        let id = sched.current_task_id().unwrap_or(TaskId(0));
        let code = sched.current_task().map(|t| t.exit_code).unwrap_or(0);
        (id, code)
    };

    // Publish the exit before preparing context switch.
    crate::services::record_exit(exiting_id, exit_code);

    x86_64::instructions::interrupts::without_interrupts(|| {
        let (prev, next) = {
            let mut sched = SCHEDULER.lock();
            sched.prepare_terminate()
        };

        if let (Some(prev_ptr), Some(next_ptr)) = (prev, next) {
            unsafe {
                switch_context(prev_ptr, next_ptr);
            }
        }
    });

    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityTable;
    use crate::task::{TaskId, TaskState};

    extern "C" fn dummy_test_entry() -> ! {
        loop {
            core::hint::spin_loop();
        }
    }

    #[test]
    fn normal_child_exit_and_wait_reap() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("parent_task", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1);
        assert_ne!(parent_id, TaskId(0));
        let child_id = sched.spawn_with_parent("child_task", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, parent_id);
        assert_ne!(child_id, TaskId(0));

        let initial_tasks = sched.task_count();
        assert_eq!(sched.get_task(child_id).unwrap().state, TaskState::Ready);
        assert_eq!(sched.get_task(child_id).unwrap().parent_id, parent_id);

        // Child terminates
        sched.terminate_child(parent_id, child_id, 42).expect("child termination");
        assert_eq!(sched.get_task(child_id).unwrap().state, TaskState::Zombie);
        assert_eq!(sched.get_task(child_id).unwrap().exit_code, 42);

        // Active task count excludes zombies, but task count preserves TCB for parent reaping
        assert_eq!(sched.task_count(), initial_tasks);

        // Wrong parent cannot reap child
        let wrong_parent = TaskId(999);
        assert!(sched.reap_child(wrong_parent, child_id).is_err());

        // Correct parent reaps child
        let exit_code = sched.reap_child(parent_id, child_id).expect("parent reaps child");
        assert_eq!(exit_code, 42);

        // Child is removed from scheduler tasks
        assert_eq!(sched.task_count(), initial_tasks - 1);
        assert!(sched.get_task(child_id).is_none());

        // Process table record for child is reaped
        assert!(
            crate::task::process::process_for_task(child_id).is_none(),
            "reap_child left {:?} bound to {:?}",
            process_leftover(child_id),
            child_id
        );

        // Clean up parent
        let _ = sched.reap_child(TaskId(0), parent_id);
    }

    /// Drives `reap_orphans` until it stops making progress, bounded.
    ///
    /// WHY this exists: losing the process or thread table race defers a reap to
    /// a later tick by design, and every test in this binary shares those tables
    /// while running in parallel. Asserting that one particular call reaps a
    /// fixed count was therefore only ever true when nothing else happened to
    /// hold a table at that instant.
    ///
    /// The property under test is that orphans are eventually reclaimed and each
    /// is counted exactly once, so this accumulates until the reaper goes quiet
    /// and keeps the bound finite so a failure cannot hang the suite.
    fn reap_orphans_until_quiet(sched: &mut Scheduler) -> usize {
        let mut total = 0;
        for _ in 0..64 {
            let n = sched.reap_orphans();
            if n == 0 {
                break;
            }
            total += n;
        }
        total
    }

    /// The process record still bound to `task`, for assertion messages.
    ///
    /// WHY the leftover is named instead of only its absence asserted:
    /// `TaskId::new()` is one monotone counter for the whole test binary, so a
    /// record some earlier case leaked against a hand-picked id is still bound
    /// when a later `spawn` is handed that same id. Reporting the surviving
    /// record turns that aliasing into a readable failure instead of a bare
    /// `is_none()` that says nothing about who left it there.
    fn process_leftover(task: TaskId) -> Option<crate::task::process::ProcessRecord> {
        crate::task::process::process_for_task(task)
            .and_then(crate::task::process::record_for_process)
    }

    #[test]
    fn repeated_exit_reap_returns_error_and_does_not_double_free() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("parent_task2", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1);
        let child_id = sched.spawn_with_parent("child_task2", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, parent_id);

        sched.terminate_child(parent_id, child_id, 7).expect("terminate child");
        assert_eq!(sched.reap_child(parent_id, child_id).expect("first reap"), 7);

        // Repeated reap MUST fail cleanly without panic or corruption
        let second_reap = sched.reap_child(parent_id, child_id);
        assert!(second_reap.is_err(), "second reap must return error");

        // Process reap directly on already reaped task must fail cleanly
        assert!(crate::task::process::reap(child_id).is_err());

        // Clean up parent
        let _ = sched.reap_child(TaskId(0), parent_id);
    }

    #[test]
    fn parent_exit_before_child_reparents_and_reaps_orphans() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("parent_task3", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1);
        let child_id = sched.spawn_with_parent("child_task3", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, parent_id);

        // Parent exits before child
        sched.terminate_task(parent_id, 0).expect("parent exit");

        // Child must be reparented to TaskId(0)
        assert_eq!(sched.get_task(child_id).unwrap().parent_id, TaskId(0));

        // While child is still running (Ready), reap_orphans must NOT touch it
        let reaped = reap_orphans_until_quiet(&mut sched);
        assert_eq!(reaped, 1); // Only the dead parent (whose parent is TaskId(0)) is reaped
        assert!(sched.get_task(child_id).is_some());
        assert_eq!(sched.get_task(child_id).unwrap().state, TaskState::Ready);

        // Now child exits
        sched.terminate_task(child_id, 99).expect("child exit");
        assert_eq!(sched.get_task(child_id).unwrap().state, TaskState::Zombie);
        assert_eq!(sched.get_task(child_id).unwrap().parent_id, TaskId(0));

        // Kernel orphan reaper must reclaim child
        let reaped_child = reap_orphans_until_quiet(&mut sched);
        assert_eq!(reaped_child, 1);
        assert!(sched.get_task(child_id).is_none());
        assert!(
            crate::task::process::process_for_task(child_id).is_none(),
            "orphan reaper left {:?} bound to {:?}",
            process_leftover(child_id),
            child_id
        );
    }

    #[test]
    fn child_exit_before_parent_reparents_and_reaps_when_parent_dies() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("parent_task4", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1);
        let child_id = sched.spawn_with_parent("child_task4", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, parent_id);

        // Child exits first and becomes Zombie
        sched.terminate_child(parent_id, child_id, 15).expect("child exit");
        assert_eq!(sched.get_task(child_id).unwrap().state, TaskState::Zombie);
        assert_eq!(sched.get_task(child_id).unwrap().parent_id, parent_id);

        // Parent dies without calling waitpid
        sched.terminate_task(parent_id, 1).expect("parent exit");

        // Child is reparented to TaskId(0)
        assert_eq!(sched.get_task(child_id).unwrap().parent_id, TaskId(0));

        // reap_orphans must reclaim both parent and child
        let reaped = reap_orphans_until_quiet(&mut sched);
        assert_eq!(reaped, 2);
        assert!(sched.get_task(child_id).is_none());
        assert!(sched.get_task(parent_id).is_none());
        assert!(
            crate::task::process::process_for_task(child_id).is_none(),
            "orphan reaper left {:?} bound to {:?}",
            process_leftover(child_id),
            child_id
        );
        assert!(
            crate::task::process::process_for_task(parent_id).is_none(),
            "orphan reaper left {:?} bound to {:?}",
            process_leftover(parent_id),
            parent_id
        );
    }

    #[test]
    fn development_bridge_task_exit_reaped_by_kernel() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        // Development bridge launches apps with parent = TaskId(0)
        let dev_task = sched.spawn_with_parent("dev_app", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, TaskId(0));
        assert_eq!(sched.get_task(dev_task).unwrap().parent_id, TaskId(0));
        assert!(crate::task::process::process_for_task(dev_task).is_some());

        // Dev app exits
        sched.terminate_task(dev_task, 0).expect("dev task exit");
        assert_eq!(sched.get_task(dev_task).unwrap().state, TaskState::Zombie);

        // reap_orphans reclaims it
        let reaped = reap_orphans_until_quiet(&mut sched);
        assert_eq!(reaped, 1);
        assert!(sched.get_task(dev_task).is_none());
        assert!(
            crate::task::process::process_for_task(dev_task).is_none(),
            "the dev bridge app {:?} still binds process {:?}",
            dev_task,
            process_leftover(dev_task),
        );
    }

    #[test]
    fn resource_reclamation_cycle_preserves_limits_and_no_leak() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let initial_task_count = sched.task_count();

        for _ in 0..5 {
            let parent_id = sched.spawn("cycle_parent", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1);
            let child_id = sched.spawn_with_parent("cycle_child", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, parent_id);

            sched.terminate_child(parent_id, child_id, 0).expect("terminate child");
            let _ = sched.reap_child(parent_id, child_id).expect("reap child");

            sched.terminate_task(parent_id, 0).expect("terminate parent");
            let _ = sched.reap_orphans();
        }

        assert_eq!(sched.task_count(), initial_task_count);
    }

    #[test]
    fn reap_orphans_lock_safety_when_tables_busy() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        // Spawn orphan task with parent TaskId(0)
        let orphan = sched.spawn_with_parent(
            "orphan_lock_test",
            dummy_test_entry,
            CapabilityTable::new_for_task(TaskId::new(), 16),
            1,
            TaskId(0),
        );
        sched.terminate_task(orphan, 0).expect("terminate orphan");
        assert_eq!(sched.get_task(orphan).unwrap().state, TaskState::Zombie);

        // When PROCESS_TABLE is locked (e.g. by an interrupted thread on this CPU),
        // try_reap must report ReapError::TablesBusy and reap_orphans must NOT deadlock or panic.
        {
            let _busy_processes = crate::task::process::PROCESS_TABLE.lock();
            let reaped = sched.reap_orphans();
            assert_eq!(reaped, 0, "must skip task when process table is locked");
            assert!(sched.get_task(orphan).is_some(), "task must remain in table for later reaping");
        }

        // When THREAD_TABLE is locked, try_reap must similarly skip without deadlocking.
        {
            let _busy_threads = crate::task::process::THREAD_TABLE.lock();
            let reaped = sched.reap_orphans();
            assert_eq!(reaped, 0, "must skip task when thread table is locked");
            assert!(sched.get_task(orphan).is_some());
        }

        // Once both tables are unlocked, reap_orphans succeeds cleanly.
        let reaped = reap_orphans_until_quiet(&mut sched);
        assert_eq!(reaped, 1, "must reap task when tables are unlocked");
        assert!(sched.get_task(orphan).is_none());
        assert!(crate::task::process::process_for_task(orphan).is_none());
    }

    /// A zombie that loses several races on the process and thread tables must
    /// survive every deferral and still be reclaimed exactly once.
    ///
    /// WHY this matters: the reaping attempt is made from the tick loop, where
    /// losing the table race is normal rather than exceptional. If a deferral
    /// dropped the zombie instead of retrying, the task control block would be
    /// freed while its process and thread table entries survived, which is the
    /// silent leak the bounded retry exists to avoid. If a second pass double
    /// counted it, the accounting would overstate how much was reclaimed.
    #[test]
    fn a_zombie_deferred_by_busy_tables_is_reaped_exactly_once() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let baseline = sched.task_count();
        let orphan = sched.spawn_with_parent(
            "orphan_deferred_reap",
            dummy_test_entry,
            CapabilityTable::new_for_task(TaskId::new(), 16),
            1,
            TaskId(0),
        );
        sched.terminate_task(orphan, 7).expect("terminate orphan");
        assert_eq!(sched.get_task(orphan).unwrap().state, TaskState::Zombie);
        assert_eq!(sched.task_count(), baseline + 1);

        // Several ticks in which the orphan loses the table race. Each one must
        // defer: report nothing reaped and keep the task table entry intact.
        // The two contention sources are exercised separately because they are
        // guarded by different lock types.
        for _ in 0..3 {
            let held = crate::task::process::PROCESS_TABLE.lock();
            assert_eq!(sched.reap_orphans(), 0, "must report nothing reaped while PROCESS_TABLE is held");
            assert!(sched.get_task(orphan).is_some(), "a deferred zombie must keep its task table entry");
            drop(held);

            let held = crate::task::process::THREAD_TABLE.lock();
            assert_eq!(sched.reap_orphans(), 0, "must report nothing reaped while THREAD_TABLE is held");
            assert!(sched.get_task(orphan).is_some(), "a deferred zombie must keep its task table entry");
            drop(held);
        }
        assert_eq!(
            sched.task_count(),
            baseline + 1,
            "a deferred zombie must remain counted in the task table"
        );

        // The tick after contention ends reclaims it, and reclaims it once.
        assert_eq!(
            reap_orphans_until_quiet(&mut sched), 1,
            "the deferred zombie must be reclaimed once released"
        );
        assert!(sched.get_task(orphan).is_none(), "the task control block must be dropped");
        assert!(
            crate::task::process::process_for_task(orphan).is_none(),
            "the process record must be released, not leaked"
        );
        assert_eq!(sched.task_count(), baseline, "task table must return to its baseline size");

        // Further ticks must not re-count a zombie that is already gone.
        assert_eq!(sched.reap_orphans(), 0, "a reclaimed zombie must not be counted twice");
        assert_eq!(sched.reap_orphans(), 0, "a reclaimed zombie must not be counted twice");
    }

    /// A deferred child stays a zombie and stays owned, so its parent can retry.
    ///
    /// WHY this case exists at all: `reap_child` drives `waitpid`, `app_stop` and
    /// `app_restart`, and it used to drop the task control block after a `let _ =`
    /// on the process side. `process::reap` reports `TablesBusy` after its 10,000
    /// spins run out, so a parent could be told a child was reaped while
    /// `PROCESS_TABLE` still held that child's record and `THREAD_TABLE` still
    /// bound its id. Because task ids come from one monotone counter, the next
    /// spawn handed that id inherits a record it did not create -- a leak of one
    /// of `MAX_PROCESSES` = 128 slots plus an aliased identity, which is what the
    /// assertions below name. Restore the `let _ =` and the first attempt fails:
    /// it returns `Ok(42)` and drops the child while its record is still held.
    #[test]
    fn reap_child_that_cannot_release_the_process_record_reports_no_success() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("waitpid_parent", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1);
        let child_id = sched.spawn_with_parent("waitpid_child", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 1, parent_id);
        let _child_leader = crate::task::process::OwnedLeader::adopted(child_id);
        let _parent_leader = crate::task::process::OwnedLeader::adopted(parent_id);

        sched.terminate_child(parent_id, child_id, 42).expect("terminate child");
        let task_count_before = sched.task_count();

        {
            // The hold stands in for the interrupted table owner: on this port the
            // guest cannot be preempted out of a table critical section, so the
            // only thread-context caller that reaches `TablesBusy` is one whose
            // bounded spin genuinely ran out.
            let _busy = crate::task::process::PROCESS_TABLE.lock();
            let deferred = sched.reap_child(parent_id, child_id);
            let Err(reason) = deferred else {
                panic!("a reap that did not release the process record must not report success, got {:?}", deferred);
            };
            assert!(
                reason.contains("process record"),
                "the error must name what failed, got {:?}",
                reason
            );
            assert_eq!(
                sched.get_task(child_id).map(|task| task.state),
                Some(TaskState::Zombie),
                "a deferred child must keep its task table entry so its parent can retry"
            );
            assert_eq!(sched.task_count(), task_count_before, "a deferred reap must not remove a task");
        }

        assert_eq!(
            sched.reap_child(parent_id, child_id),
            Ok(42),
            "the retry once the table is free must reap the child"
        );
        assert!(sched.get_task(child_id).is_none(), "the reaped task control block must be dropped");
        assert_eq!(sched.task_count(), task_count_before - 1);
        assert!(
            crate::task::process::process_for_task(child_id).is_none(),
            "the committed reap must leave {:?} bound to nothing, still holding {:?}",
            child_id,
            process_leftover(child_id),
        );
    }

    #[test]
    fn test_ui_pipeline_services_pinned_to_realtime() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        // WHY the ids are spent first: `configure_task_ui_pipeline` pins the boot UI
        // services by position as well as by name -- ids 2, 3 and 10 -- because those
        // are the positions the boot sequence gives displayd, inputd and compositor.
        // Ids come from one process-global counter, so an app spawned by a case that
        // runs early can be handed one of those positions and is born RealTime.
        while TaskId::new().0 <= BOOT_SERVICE_ID_CEILING {}

        let displayd_id = sched.spawn("displayd", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        let inputd_id = sched.spawn("inputd", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        let comp_id = sched.spawn("compositor", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        let normal_app = sched.spawn("notes", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 6);

        // Verify that displayd, inputd, and compositor are recognized as real-time UI pipeline tasks
        let displayd_task = sched.get_task(displayd_id).unwrap();
        assert_eq!(displayd_task.sched_class, crate::task::SchedulingClass::RealTime);
        assert_eq!(displayd_task.priority, PRIORITY_REALTIME);
        assert_eq!(displayd_task.frame_budget_ns, VSYNC_BUDGET_60HZ_NS);
        assert!(sched.is_realtime_ui_task(displayd_task));

        let inputd_task = sched.get_task(inputd_id).unwrap();
        assert_eq!(inputd_task.sched_class, crate::task::SchedulingClass::RealTime);
        assert_eq!(inputd_task.priority, PRIORITY_REALTIME);
        assert!(sched.is_realtime_ui_task(inputd_task));

        let comp_task = sched.get_task(comp_id).unwrap();
        assert_eq!(comp_task.sched_class, crate::task::SchedulingClass::RealTime);
        assert_eq!(comp_task.priority, PRIORITY_REALTIME);
        assert!(sched.is_realtime_ui_task(comp_task));

        let app_task = sched.get_task(normal_app).unwrap();
        assert_eq!(app_task.sched_class, crate::task::SchedulingClass::Normal);
        assert_eq!(app_task.priority, 6);
        assert!(!sched.is_realtime_ui_task(app_task));
    }

    #[test]
    fn test_foreground_task_promoted_to_realtime_and_edf() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        // WHY the ids are spent first: `set_active_foreground_task` only demotes a
        // task it treats as an app, and the production rule for that is `id.0 > 10`
        // because task ids 1..10 are the boot services. Ids come from one
        // process-global counter, so a case that runs early is handed an id inside
        // that band, the demotion line is skipped, and the last assertion fails for
        // a reason that has nothing to do with scheduling.
        while TaskId::new().0 <= BOOT_SERVICE_ID_CEILING {}

        let app_id = sched.spawn("vault", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 6);
        assert_eq!(sched.get_task(app_id).unwrap().sched_class, crate::task::SchedulingClass::Normal);

        // Foregrounding promotes the app to RealTime class and priority
        sched.set_active_foreground_task(Some(app_id));
        let fg_task = sched.get_task(app_id).unwrap();
        assert_eq!(fg_task.sched_class, crate::task::SchedulingClass::RealTime);
        assert_eq!(fg_task.priority, PRIORITY_REALTIME);
        assert!(sched.is_realtime_ui_task(fg_task));

        // Test EDF deadline configuration (e.g. 120Hz VSync deadline <8.3ms)
        sched.set_task_deadline(app_id, 100, VSYNC_BUDGET_120HZ_NS);
        let deadline_task = sched.get_task(app_id).unwrap();
        assert_eq!(deadline_task.deadline_ticks, 100);
        assert_eq!(deadline_task.frame_budget_ns, VSYNC_BUDGET_120HZ_NS);

        // Backgrounding demotes back to Normal class
        sched.set_active_foreground_task(None);
        let bg_task = sched.get_task(app_id).unwrap();
        assert_eq!(bg_task.sched_class, crate::task::SchedulingClass::Normal);
        assert_eq!(bg_task.priority, PRIORITY_APP);
    }
}

