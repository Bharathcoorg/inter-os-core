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
use core::sync::atomic::{AtomicUsize, Ordering};
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
    /// The owner uid established at termination, while the process record was
    /// still live. `None` means no identity could be established; reclaim must
    /// not guess one. See the notification tail of `reclaim_task`.
    user_id: Option<u32>,
}

/// Terminations awaiting reclaim.
///
/// WHY a growable queue and not a fixed array: the old 64-slot array dropped
/// entries on overflow with only a serial warning, so a terminated task's
/// delegated authority, IPC endpoints and consent records survived it --
/// fail-open under load, and 64 exits inside one main-loop pass is reachable.
/// Overflow is now impossible by construction: every task-creation path
/// reserves capacity for its exit before publishing the task
/// (`reserve_cleanup_slot`), each TaskId is queued at most once (dedup below,
/// and a Zombie is never scheduled or terminated again), and draining only
/// removes entries while capacity is never shrunk. The invariant is
/// `capacity >= len + CLEANUP_RESERVATIONS`, so the push at termination never
/// allocates and can never be refused.
static DEFERRED_CLEANUP: Mutex<Vec<DeferredCleanup>> = Mutex::new(Vec::new());

/// Outstanding reclaim reservations: one per created task that has not yet
/// queued its exit. Every mutation of this counter happens under the
/// `DEFERRED_CLEANUP` lock, alongside the length it is paired with.
///
/// WHY a counter and not just `try_reserve(1)` per spawn: `try_reserve`
/// reserves relative to the CURRENT length, so reservations made while the
/// queue is empty would all be satisfied by the same single slot of headroom
/// and the second push would reallocate. Reserving for `reservations + 1`
/// keeps the invariant above true across any interleaving of spawns, exits
/// and drains.
static CLEANUP_RESERVATIONS: AtomicUsize = AtomicUsize::new(0);

/// Reserves capacity for one future reclaim entry, before the task exists.
///
/// WHY creation fails closed when this fails: a task whose exit could not be
/// queued would keep live delegated authority after termination. Refusing to
/// create it is the only outcome that leaks nothing; the reservation itself is
/// fallible (`try_reserve`), so an exhausted heap refuses the spawn instead of
/// panicking or halting inside the exit path later.
fn reserve_cleanup_slot() -> bool {
    let mut pending = DEFERRED_CLEANUP.lock();
    let needed = CLEANUP_RESERVATIONS.load(Ordering::Relaxed).saturating_add(1);
    if pending.try_reserve(needed).is_err() {
        return false;
    }
    CLEANUP_RESERVATIONS.fetch_add(1, Ordering::Relaxed);
    true
}

/// Returns a reservation a failed creation never used. Without this a refused
/// spawn would only over-reserve capacity, never under-reserve; releasing
/// keeps the bound tight.
fn release_cleanup_slot() {
    let _ = CLEANUP_RESERVATIONS.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_sub(1))
    });
}

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
///
/// `user_id` is captured by the caller at termination, never re-derived at
/// reclaim time: by then the process record may already be released.
pub fn defer_cleanup(id: TaskId, name: &'static str, user_id: Option<u32>) {
    let mut pending = DEFERRED_CLEANUP.lock();
    if pending.iter().any(|entry| entry.id == id) {
        return;
    }
    // Capacity for this entry was reserved when the task was created, so this
    // push cannot allocate and cannot be refused. The assert pins that
    // invariant in test builds; if it were ever broken, the push would go
    // through the kernel allocator whose OOM policy halts loudly (lib.rs) --
    // never a silent drop of a reclaim.
    debug_assert!(pending.len() < pending.capacity());
    let _ = CLEANUP_RESERVATIONS.try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_sub(1))
    });
    pending.push(DeferredCleanup { id, name, user_id });
}

/// Reclaims resources of tasks that terminated since the last pass.
///
/// MUST be called without `SCHEDULER` held. The hooks below may block on the
/// subsystem they own; that is safe exactly here, because the caller owns
/// nothing the scheduler needs and the timer can preempt it.
pub fn run_deferred_cleanup() {
    loop {
        let entry = DEFERRED_CLEANUP.lock().pop();
        let Some(entry) = entry else { return; };
        reclaim_task(entry);
    }
}

/// Releases everything a departed task owned outside the scheduler.
fn reclaim_task(entry: DeferredCleanup) {
    crate::ipc::cleanup_task(entry.id);
    crate::memory::cleanup_loans_for_task(entry.id);
    crate::fs::cleanup_task(entry.id);
    crate::event::cleanup_task(entry.id);
    crate::net::cleanup_task(entry.id);
    crate::drivers::input::cleanup_task(entry.id);
    crate::gui::BUFFER_MANAGER.lock().release_owned_by(entry.id.0);
    // Authority follows task lifetime. `capability::cleanup_task` revokes the
    // exiting task's delegated leases, prunes them from the kernel-wide
    // registry and drops its consent records; without this a dead task kept
    // live delegated authority and `has_permission` kept answering for it.
    crate::capability::cleanup_task(entry.id);

    // Clean up any lingering futex wait state for the departing task
    crate::task::futex::FUTEX_MANAGER.lock().cleanup_task(entry.id);

    // If this was the last thread of the process, unblock and clean up all process futex waiters
    if let Some(pid) = crate::task::process::process_for_task(entry.id) {
        if crate::task::process::live_threads_for_process(pid) <= 1 {
            let orphaned = crate::task::futex::FUTEX_MANAGER.lock().cleanup_process(pid);
            if !orphaned.is_empty() {
                // WHY a blocking lock here and a `try_lock` on the tick path:
                // `cleanup_process` has already REMOVED these waiters, so a
                // dropped unblock list cannot be retried by a later tick -- the
                // tasks would stay Blocked with nothing left to wake them. This
                // function runs in task context owning no scheduler lock (see
                // `run_deferred_cleanup`), and already takes `SCHEDULER.lock()`
                // below for the shell lookup, so waiting here is safe.
                let mut sched = SCHEDULER.lock();
                for tid in orphaned {
                    sched.unblock_task(tid);
                }
            }
        }
    }

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

    // WHY the uid is carried in the entry and never re-derived here: reclaim
    // runs after the process record may already have been released -- the
    // `process_for_task` lookup above returns None on that normal path -- so
    // `user_for_task` here would answer None for a task whose owner was known
    // at exit. The old `unwrap_or(0)` turned that into cancelling user 0's
    // notifications for the package: a different account's data destroyed
    // because the owner could not be resolved. `None` means no identity could
    // be established at termination; such a task could never have posted its
    // own notifications (syscall NotificationPost requires `caller_user_id`,
    // the same accessor, and refuses None), so nothing of its own is left
    // behind, and the failure is surfaced rather than guessed at. Neither
    // uid 0 nor `cancel_all_for_package_all_users` is a safe default: both
    // destroy records belonging to other accounts.
    match entry.user_id {
        Some(user_id) => {
            crate::notification::NOTIFICATION_MANAGER
                .lock()
                .cancel_all_for_package(entry.name, user_id);
        }
        None => crate::serial_println!(
            "[LIFECYCLE] task {} ({}) exited without an establishable user identity; its package notifications were not cancelled",
            entry.id.0,
            entry.name
        ),
    }
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
        // Even the primordial task must have a reclaim slot reserved before it
        // exists. A failed reservation leaves the scheduler uninitialized, so
        // nothing is scheduled -- fail closed rather than run a task whose
        // exit could not be queued.
        if !reserve_cleanup_slot() {
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
        // Reserve the exit's reclaim slot before the task exists; see
        // `reserve_cleanup_slot`. A refused spawn creates no reclaim debt.
        if !reserve_cleanup_slot() {
            return TaskId(0);
        }
        let mut tcb = TaskControlBlock::new(name, entry_point, cap_table, priority);
        tcb.parent_id = parent_id;
        let id = tcb.id;
        let Some(process_id) = crate::task::process::register_leader(id, parent_id) else {
            return TaskId(0);
        };
        tcb.process_id = Some(process_id);
        self.tasks.push(tcb);
        self.configure_task_ui_pipeline(id);
        let ipc_registered = crate::ipc::IPC_ROUTER.lock().register_task(id);
        if !ipc_registered {
            self.tasks.pop();
            crate::task::process::try_reap(id).ok();
            release_cleanup_slot();
            return TaskId(0);
        }
        id
    }

    /// Spawns an existing TaskControlBlock (e.g. userspace process with isolated page table).
    pub fn spawn_task(&mut self, mut tcb: TaskControlBlock) -> TaskId {
        // Reserve the exit's reclaim slot before the task exists; see
        // `reserve_cleanup_slot`.
        if !reserve_cleanup_slot() {
            return TaskId(0);
        }
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
        let ipc_registered = crate::ipc::IPC_ROUTER.lock().register_task(id);
        if !ipc_registered {
            self.tasks.pop();
            crate::task::process::try_reap(id).ok();
            release_cleanup_slot();
            return TaskId(0);
        }
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
        mut cap_table: CapabilityTable,
        priority: u8,
    ) -> Result<TaskId, &'static str> {
        // Every user thread terminates through `defer_cleanup` too, so it
        // needs its own reserved reclaim slot before it exists.
        if !reserve_cleanup_slot() {
            return Err("cleanup capacity unavailable");
        }
        crate::task::process::reserve_thread(process_id)?;
        let shared_space = match self.tasks.iter()
            .find(|t| t.process_id == Some(process_id))
            .and_then(|t| t.page_table.clone()) {
            Some(space) => space,
            None => {
                crate::task::process::cancel_thread_reservation(process_id);
                release_cleanup_slot();
                return Err("process has no userspace address space");
            }
        };

        cap_table.rekey_all();
        let mut tcb = TaskControlBlock::new("user_thread", crate::task::user::user_thread_entry, cap_table, priority);
        let id = tcb.id;
        let custom_stack = user_stack != 0;

        let (user_stack_top, cr3_frame) = if custom_stack {
            let pt = shared_space.lock();
            (user_stack, pt.frame())
        } else {
            if let Err(e) = crate::task::process::reserve_address_space(process_id, crate::task::user::USER_STACK_SIZE) {
                crate::task::process::cancel_thread_reservation(process_id);
                release_cleanup_slot();
                return Err(e);
            }
            let mut pt = shared_space.lock();
            let frame = pt.frame();
            match crate::task::user::map_user_thread_stack(&mut pt, id) {
                Ok(top) => (top, frame),
                Err(e) => {
                    crate::task::process::cancel_thread_reservation(process_id);
                    crate::task::process::release_address_space(process_id, crate::task::user::USER_STACK_SIZE);
                    release_cleanup_slot();
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
        let leader_identity = self.tasks.iter().find(|t| t.process_id == Some(process_id)).and_then(|t| t.security_identity);
        tcb.security_identity = leader_identity;

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
            release_cleanup_slot();
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
            crate::task::process::try_reap(id).ok();
            release_cleanup_slot();
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

    /// Removes a TCB a test case spawned, returning the scheduler it inherited.
    ///
    /// WHY the production reaper cannot do this: `reap_terminated` deliberately
    /// skips the current task, and a case that spawns into an empty scheduler
    /// leaves its own task AT `current_idx` -- so the TCB survives every reap.
    /// The next case then inherits an application task as "current", its
    /// capability checks resolve against that identity instead of its own, and
    /// it passes or fails depending on suite order.
    #[cfg(test)]
    pub(crate) fn drop_test_task(&mut self, task: TaskId) -> bool {
        let Some(i) = self.tasks.iter().position(|t| t.id == task) else {
            return false;
        };
        self.tasks.remove(i);
        if i < self.current_idx {
            self.current_idx -= 1;
        }
        if self.current_idx >= self.tasks.len() {
            self.current_idx = self.tasks.len().saturating_sub(1);
        }
        true
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
        // WHY the uid is captured here and not at reclaim: the process record
        // is still live at termination, and reclaim may run after it has been
        // released. SCHEDULER -> PROCESS_TABLE is the order this path already
        // uses (`mark_zombie` just above and `release_address_space` earlier
        // take PROCESS_TABLE under SCHEDULER); no path holds a process table
        // across a blocking SCHEDULER lock -- process.rs drops its table
        // guards before touching the scheduler or uses try_lock -- so the
        // capture introduces no new lock-order edge. `user_for_task` reads
        // THREAD_TABLE with a bounded fail-safe try_lock spin, and a None
        // answer is carried as None, never defaulted.
        defer_cleanup(
            task_id,
            self.tasks[current_idx].name,
            crate::task::process::user_for_task(task_id),
        );

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
            self.park_current_if_budget_suspended();
            self.prepare_switch()
        } else {
            (None, None)
        }
    }

    /// Parks the current task when the background budget suspended its process.
    ///
    /// WHY a pending kill is exempt: `request_kill` unblocks a parked task so it
    /// can run to the cooperative exit gate at syscall return. Re-parking it here
    /// because its process is still Suspended would strand it in front of that
    /// gate -- the kill would never complete, and the task's capabilities, IPC
    /// endpoints and mappings would never be reclaimed. Termination outranks the
    /// budget: the task is leaving anyway.
    fn park_current_if_budget_suspended(&mut self) {
        let Some(current) = self.tasks.get(self.current_idx) else { return };
        let suspended = current.process_id
            .and_then(crate::task::process::record_for_process)
            .map(|r| r.state == crate::task::process::ProcessState::Suspended)
            .unwrap_or(false);
        if !suspended || current.pending_kill {
            return;
        }
        let current_id = current.id;
        if let Some(task) = self.tasks.iter_mut().find(|t| t.id == current_id) {
            task.state = TaskState::Blocked;
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
        task.cap_table.revoke_all();
        task.exit_code = exit_code;
        task.state = TaskState::Zombie;
        crate::task::process::mark_zombie(id, exit_code);
        // Capture the owner while the process record is live; see the same
        // capture in `prepare_terminate` for the lock-order argument.
        defer_cleanup(id, task_name, crate::task::process::user_for_task(id));

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

    /// Marks a task for cooperative termination on syscall return or next scheduling point.
    /// Allows in-flight syscalls to cleanly unwind kernel stack frames and release spinlocks.
    /// If the task is currently blocked, transitions it to Ready so the scheduler can dispatch it
    /// to complete cooperative unwinding.
    pub fn request_kill(&mut self, id: TaskId, exit_code: i32) -> Result<(), &'static str> {
        let task = self.tasks.iter_mut().find(|task| task.id == id).ok_or("task not found")?;
        if matches!(task.state, TaskState::Zombie | TaskState::Terminated) {
            return Ok(());
        }
        task.pending_kill = true;
        task.exit_code = exit_code;
        if matches!(task.state, TaskState::Blocked | TaskState::WaitPid(_)) {
            task.state = TaskState::Ready;
        }
        Ok(())
    }

    /// Checks whether the currently executing task has a pending kill request.
    pub fn is_current_pending_kill(&self) -> bool {
        self.current_task().map_or(false, |t| t.pending_kill)
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

    /// Reaps any terminated child of `parent_id`.
    pub fn reap_any_child(&mut self, parent_id: TaskId) -> Result<(TaskId, i32), &'static str> {
        let index = self.tasks.iter().position(|task| {
            task.parent_id == parent_id
                && matches!(task.state, TaskState::Zombie | TaskState::Terminated)
        }).ok_or("no child ready")?;
        let child_id = self.tasks[index].id;
        let exit_code = self.tasks[index].exit_code;
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
        Ok((child_id, exit_code))
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
                // `try_kernel_pml4` rather than `kernel_pml4`: this is telemetry,
                // and a process table that panicked the guest it was reporting on
                // would be a very expensive diagnostic. `0` is the honest answer
                // for "no page table has been recorded yet", which is the only
                // case where the frame is absent.
                .unwrap_or_else(|| crate::memory::try_kernel_pml4().map(|f| f.start_address().as_u64()).unwrap_or(0));
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
        let expired = fm.take_timeouts(ticks);
        let mut sched_guard = SCHEDULER.try_lock();
        dispatch_futex_timeouts(&mut fm, expired, sched_guard.as_deref_mut());
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

/// Delivers expired futex waiters to the scheduler, or hands their deadlines back.
///
/// WHY the restore rather than a dropped list: `take_timeouts` CONSUMES each
/// deadline it reports. If the wake cannot be delivered, a consumed deadline means
/// no later tick will ever report that waiter again, so it stays Blocked until an
/// explicit wake or process exit even though its timeout expired. Restoring re-arms
/// it for the next tick -- the same retry contract `event::on_timer_tick` keeps with
/// `is_waiting`.
///
/// Two cases cannot be delivered:
/// * the scheduler lock is held by the task this interrupt landed on (`sched` is
///   `None`), so no task state can be touched at all;
/// * the waiter is published but its task has not reached `block_task` yet. It is
///   still Running/Ready, `unblock_task` would be a no-op, and consuming the
///   deadline would strand it the moment it parks.
///
/// A task that is gone, Zombie or Terminated has nothing left to wake: its expiry
/// is consumed so a stale entry cannot be reported on every tick.
///
/// `sched` is the guard the tick managed to take, if any. Both locks are only ever
/// `try_lock`ed here: this runs in interrupt context.
fn dispatch_futex_timeouts(
    fm: &mut crate::task::futex::FutexManager,
    expired: crate::task::futex::FutexTimeouts,
    sched: Option<&mut Scheduler>,
) {
    if expired.is_empty() {
        return;
    }
    let Some(sched) = sched else {
        fm.restore_timeouts(&expired);
        return;
    };
    for &(tid, deadline_ticks) in expired.as_slice() {
        match sched.get_task(tid).map(|t| t.state) {
            Some(TaskState::Blocked) => sched.unblock_task(tid),
            Some(TaskState::Running) | Some(TaskState::Ready) => {
                fm.restore_timeout(tid, deadline_ticks);
            }
            _ => {}
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

/// Returns whether the current task has a pending kill request waiting to be processed.
///
/// WHY this never falls back to a blocking lock: the timer handler calls it on the
/// return-to-Ring3 path, and `SCHEDULER.lock()` in interrupt context spins forever
/// whenever the interrupt landed inside a scheduler critical section of the task
/// it interrupted -- one CPU, so no other core can release it. `crate::sync` states
/// that rule for every lock the tick path takes.
///
/// A contended answer is `false`, which postpones the kill by at most one gate: the
/// flag stays set, so the next syscall return or timer tick reads it again. The
/// task is back in Ring 3 in the meantime, and every later syscall re-runs the gate.
pub fn is_current_pending_kill() -> bool {
    for _ in 0..64 {
        if let Some(s) = SCHEDULER.try_lock() {
            return s.is_current_pending_kill();
        }
        core::hint::spin_loop();
    }
    false
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

    #[test]
    fn test_cooperative_pending_kill_flag() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let victim_id = sched.spawn("victim", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        assert!(!sched.get_task(victim_id).unwrap().pending_kill);

        // Request kill cooperatively
        sched.request_kill(victim_id, -15).expect("request kill");
        let task = sched.get_task(victim_id).unwrap();
        assert!(task.pending_kill);
        assert_eq!(task.exit_code, -15);
    }

    #[test]
    fn test_cooperative_kill_lifecycle_and_reap() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("parent", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        let child_id = sched.spawn_with_parent("child", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5, parent_id);

        // Cooperative kill requested with exit code 42
        sched.request_kill(child_id, 42).expect("request kill");
        let task = sched.get_task(child_id).unwrap();
        assert!(task.pending_kill);
        assert_eq!(task.exit_code, 42);

        // Child task terminates into Zombie
        sched.terminate_task(child_id, 42).expect("child terminates");
        assert_eq!(sched.get_task(child_id).unwrap().state, TaskState::Zombie);

        // Parent reaps child and receives the exact exit code
        let exit_code = sched.reap_child(parent_id, child_id).expect("reap child");
        assert_eq!(exit_code, 42);

        // Child is now completely removed from task list
        assert!(sched.get_task(child_id).is_none());
    }

    #[test]
    fn test_cooperative_kill_blocked_task_unblocked() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let victim_id = sched.spawn("blocked_victim", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        // Put task into Blocked state (e.g. waiting for IPC or I/O)
        let task = sched.tasks.iter_mut().find(|t| t.id == victim_id).unwrap();
        task.state = TaskState::Blocked;

        // Request kill while task is blocked
        sched.request_kill(victim_id, 9).expect("request kill on blocked task");

        // The task MUST be unblocked (Ready) so the scheduler can dispatch it to unwind cleanly
        let task_after = sched.get_task(victim_id).unwrap();
        assert_eq!(task_after.state, TaskState::Ready);
        assert!(task_after.pending_kill);
        assert_eq!(task_after.exit_code, 9);
    }

    #[test]
    fn test_cooperative_kill_waitpid_task_unblocks() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let parent_id = sched.spawn("parent_waiting", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        let child_id = sched.spawn_with_parent("child_running", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5, parent_id);

        // Put parent task into WaitPid state waiting for child
        {
            let parent = sched.tasks.iter_mut().find(|t| t.id == parent_id).unwrap();
            parent.state = TaskState::WaitPid(child_id.0);
        }

        // Request kill while task is in WaitPid
        sched.request_kill(parent_id, -9).expect("request kill on waitpid task");

        // The parent MUST be unblocked (Ready) so it can execute the cooperative unwind gate
        let parent_after = sched.get_task(parent_id).unwrap();
        assert_eq!(parent_after.state, TaskState::Ready);
        assert!(parent_after.pending_kill);
        assert_eq!(parent_after.exit_code, -9);

        // Terminate cleanly to Zombie and verify parent/child cleanup
        sched.terminate_task(parent_id, -9).expect("terminate unblocked parent");
        assert_eq!(sched.get_task(parent_id).unwrap().state, TaskState::Zombie);
        let exit_code = sched.reap_child(TaskId(0), parent_id).expect("reap parent");
        assert_eq!(exit_code, -9);
    }

    #[test]
    fn test_cooperative_kill_idempotent_and_terminated() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));

        let victim_id = sched.spawn("victim", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);

        // Repeated requests must be idempotent and succeed
        assert_eq!(sched.request_kill(victim_id, 15), Ok(()));
        assert_eq!(sched.request_kill(victim_id, 15), Ok(()));

        // Terminate task into Terminated state
        sched.terminate_task(victim_id, 15).expect("terminate");

        // Calling request_kill on an already-terminated task is safe and idempotent
        assert_eq!(sched.request_kill(victim_id, 15), Ok(()));
    }

    #[test]
    fn test_cooperative_kill_lock_unwinding_simulation() {
        // Simulates the syscall unwinding gate:
        // A lock held during syscall logic must be dropped before the pending_kill check.
        static TEST_MUTEX: spin::Mutex<u32> = spin::Mutex::new(100);

        // 1. Syscall execution begins: acquire lock
        {
            let mut guard = TEST_MUTEX.lock();
            *guard += 1;
            assert_eq!(*guard, 101);
            // Guard goes out of scope here (stack frame unwinds)
        }

        // 2. Syscall return boundary reached: lock is verified unlocked
        assert!(TEST_MUTEX.try_lock().is_some(), "Lock must be released before termination gate");

        // 3. Syscall exit gate evaluates is_current_pending_kill() without deadlocking
        // or holding the kernel test lock.
    }

    // ------------------------------------------------------------------
    // Futex timeout delivery on the tick path (H8).
    //
    // `dispatch_futex_timeouts` is the whole of the tick's futex decision, and
    // `None` is exactly what `SCHEDULER.try_lock()` yields when the interrupt
    // landed on a task holding the scheduler. A local scheduler and manager keep
    // the case deterministic: the global ones belong to whichever test runs next.
    // ------------------------------------------------------------------

    /// Spawns one task, parks it Blocked, and registers a futex waiter for it.
    fn parked_futex_waiter(
        sched: &mut Scheduler,
        fm: &mut crate::task::futex::FutexManager,
        name: &'static str,
        deadline_ticks: u64,
    ) -> (TaskId, crate::task::process::ProcessId, u64) {
        let tid = sched.spawn(name, dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        assert_ne!(tid, TaskId(0));
        let pid = sched.get_task(tid).unwrap().process_id.expect("spawn registered a process");
        let uaddr = 0x9000_1000;
        sched.block_task(tid);
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Blocked);
        fm.add_waiter(tid, pid, uaddr, deadline_ticks);
        (tid, pid, uaddr)
    }

    /// Releases the process, thread and IPC records a case registered globally.
    ///
    /// WHY the IPC reclaim is explicit: host cases never run `run_deferred_cleanup`,
    /// so `terminate_task` only queues the reclaim. Without this the mailbox and
    /// inbound endpoint `spawn` registered would outlive the case in the global
    /// router, and the next case to enumerate endpoints would see a dead task's.
    fn retire(sched: &mut Scheduler, tid: TaskId) {
        let idx = sched.tasks.iter().position(|t| t.id == tid).unwrap();
        if sched.current_idx == idx {
            sched.current_idx = 0;
        }
        crate::ipc::cleanup_task(tid);
        let _ = crate::task::process::wake(tid);
        let _ = sched.terminate_task(tid, 0);
        let _ = sched.reap_child(TaskId(0), tid);
    }

    #[test]
    fn futex_timeout_unblocks_a_blocked_waiter() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let (tid, _pid, _uaddr) = parked_futex_waiter(&mut sched, &mut fm, "futex_timeout_ok", 100);

        let expired = fm.take_timeouts(150);
        assert_eq!(expired.as_slice(), &[(tid, 100)][..]);
        dispatch_futex_timeouts(&mut fm, expired, Some(&mut sched));

        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready);
        fm.cleanup_task(tid);
        retire(&mut sched, tid);
    }

    /// H8: a contended scheduler lock must not consume the timeout. The waiter
    /// stays Blocked on that tick and still times out on a later one.
    #[test]
    fn contended_scheduler_lock_restores_the_deadline_for_a_later_tick() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let (tid, _pid, _uaddr) = parked_futex_waiter(&mut sched, &mut fm, "futex_timeout_contended", 100);

        // Tick 1: the scheduler lock is held elsewhere, so the wake cannot land.
        let first = fm.take_timeouts(150);
        assert_eq!(first.len(), 1);
        dispatch_futex_timeouts(&mut fm, first, None);
        assert_eq!(
            sched.get_task(tid).unwrap().state,
            TaskState::Blocked,
            "a contended tick must not claim it delivered the wake"
        );

        // Tick 2, still contended: the waiter must be reported AGAIN. With the
        // deadline dropped this batch is empty and the waiter sleeps forever.
        let second = fm.take_timeouts(151);
        assert_eq!(
            second.as_slice(),
            &[(tid, 100)][..],
            "the consumed deadline was dropped: no later tick can time this waiter out"
        );
        dispatch_futex_timeouts(&mut fm, second, None);
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Blocked);

        // Tick 3: the lock is free and the timeout is finally delivered.
        let third = fm.take_timeouts(152);
        assert_eq!(third.len(), 1);
        dispatch_futex_timeouts(&mut fm, third, Some(&mut sched));
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready);

        fm.cleanup_task(tid);
        retire(&mut sched, tid);
    }

    /// A waiter whose deadline has not passed is neither woken nor re-armed by a
    /// contended tick.
    #[test]
    fn contended_tick_leaves_an_unexpired_waiter_alone() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let (tid, _pid, _uaddr) = parked_futex_waiter(&mut sched, &mut fm, "futex_not_expired", 500);

        let expired = fm.take_timeouts(150);
        assert!(expired.is_empty());
        dispatch_futex_timeouts(&mut fm, expired, None);
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Blocked);

        let later = fm.take_timeouts(500);
        assert_eq!(later.as_slice(), &[(tid, 500)][..], "the original deadline must survive untouched");
        fm.cleanup_task(tid);
        retire(&mut sched, tid);
    }

    /// A waiter can be published before its task reaches `block_task` (the tick
    /// preempted it in between). `unblock_task` is a no-op on a Running/Ready
    /// task, so consuming the deadline there would strand it as soon as it parks.
    #[test]
    fn timeout_of_a_waiter_that_has_not_parked_yet_is_retried() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let tid = sched.spawn("futex_not_parked", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        let pid = sched.get_task(tid).unwrap().process_id.expect("process registered");
        let uaddr = 0x9000_1000;
        fm.add_waiter(tid, pid, uaddr, 100);
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready);

        let expired = fm.take_timeouts(150);
        assert_eq!(expired.len(), 1);
        dispatch_futex_timeouts(&mut fm, expired, Some(&mut sched));
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready);

        // The deadline must still be armed for the tick that lands after parking.
        let again = fm.take_timeouts(151);
        assert_eq!(
            again.as_slice(),
            &[(tid, 100)][..],
            "the expiry was consumed while the task was still runnable"
        );

        sched.block_task(tid);
        dispatch_futex_timeouts(&mut fm, again, Some(&mut sched));
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready, "the parked waiter must time out");

        fm.cleanup_task(tid);
        retire(&mut sched, tid);
    }

    /// An expired waiter whose task already died is consumed, not re-reported on
    /// every tick, and the wake must not resurrect the zombie.
    #[test]
    fn expiry_of_a_dead_waiter_is_consumed_and_cannot_resurrect_it() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let (tid, _pid, _uaddr) = parked_futex_waiter(&mut sched, &mut fm, "futex_dead_waiter", 100);
        sched.terminate_task(tid, 0).expect("terminate the parked waiter");

        let expired = fm.take_timeouts(150);
        assert_eq!(expired.len(), 1);
        dispatch_futex_timeouts(&mut fm, expired, Some(&mut sched));
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Zombie);
        assert!(
            fm.take_timeouts(151).is_empty(),
            "a stale expiry must not be re-reported on every tick"
        );

        fm.cleanup_task(tid);
        retire(&mut sched, tid);
    }

    // ------------------------------------------------------------------
    // Termination of a task parked in a futex wait.
    // ------------------------------------------------------------------

    /// A task blocked on an infinite futex wait must be terminable: the kill
    /// unblocks it, its authority is revoked, its waiter is reclaimed, and a
    /// repeated wake after exit cannot resurrect it.
    #[test]
    fn kill_of_a_blocked_futex_waiter_unblocks_terminates_and_reclaims() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let (tid, pid, uaddr) = parked_futex_waiter(&mut sched, &mut fm, "futex_kill_victim", 0);

        // One live authority, so revocation on termination is observable.
        sched.get_task_mut(tid).unwrap().cap_table.insert(crate::capability::Capability {
            id: crate::capability::CapabilityId(0x00C0_FFEE_F00D),
            cap_type: crate::capability::CapabilityType::StorageRawBlock,
            rights: crate::capability::Rights::READ | crate::capability::Rights::INVOKE,
            object_id: 0,
        }).expect("insert capability");
        assert_eq!(sched.get_task(tid).unwrap().cap_table.count(), 1);

        // Cooperative kill of a Blocked waiter: it must become dispatchable.
        sched.request_kill(tid, -9).expect("kill a blocked futex waiter");
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready);
        assert!(sched.get_task(tid).unwrap().pending_kill);
        assert_eq!(sched.get_task(tid).unwrap().exit_code, -9);

        // The unwind inside sys_futex_wait: an infinite deadline never expired and
        // no wake arrived, so the waiter reports a timeout, not a successful wake.
        assert!(!fm.remove_waiter(tid));

        // Syscall-return gate -> terminate: Zombie, authority gone, not schedulable.
        sched.terminate_task(tid, -9).expect("terminate the killed waiter");
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Zombie);
        assert_eq!(sched.get_task(tid).unwrap().cap_table.count(), 0, "termination must revoke all authority");
        assert_eq!(sched.active_task_count(), 1, "a zombie must not stay schedulable");

        // Deferred reclaim drops the wait state; a late wake finds nobody.
        fm.cleanup_task(tid);
        assert!(fm.wake(pid, uaddr, 10).is_empty(), "a wake after exit must not reach the dead task");
        sched.unblock_task(tid);
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Zombie, "a late unblock must not resurrect a zombie");

        // Repeated kill requests after termination stay idempotent no-ops: the
        // state and the exit code the parent will reap are both untouched.
        assert_eq!(sched.request_kill(tid, 1), Ok(()));
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Zombie);
        assert_eq!(sched.get_task(tid).unwrap().exit_code, -9, "a late kill must not rewrite the recorded exit code");

        assert_eq!(sched.reap_child(TaskId(0), tid).expect("reap the killed waiter"), -9);
        assert!(sched.get_task(tid).is_none());
        assert!(crate::task::process::process_for_task(tid).is_none());
        crate::ipc::cleanup_task(tid);
    }

    /// A whole process exiting must not leave its parked waiters behind: the
    /// purge reports them and unblocking is applied to every one of them.
    #[test]
    fn process_exit_purges_every_parked_waiter() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let mut fm = crate::task::futex::FutexManager::new();
        let (leader, pid, uaddr) = parked_futex_waiter(&mut sched, &mut fm, "futex_proc_leader", 0);
        let sibling = sched.spawn("futex_proc_sibling", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 5);
        sched.block_task(sibling);
        fm.add_waiter(sibling, pid, uaddr, 0);

        let orphaned = fm.cleanup_process(pid);
        assert_eq!(orphaned.len(), 2);
        for tid in orphaned {
            sched.unblock_task(tid);
        }
        assert_eq!(sched.get_task(leader).unwrap().state, TaskState::Ready);
        assert_eq!(sched.get_task(sibling).unwrap().state, TaskState::Ready);
        assert!(fm.wake(pid, uaddr, 10).is_empty());

        retire(&mut sched, leader);
        retire(&mut sched, sibling);
    }

    /// A pending kill outranks the background budget: re-parking the task because
    /// its process is Suspended would strand it in front of its exit gate.
    #[test]
    fn pending_kill_is_not_reparked_by_the_suspended_budget() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        let tid = sched.spawn("budget_kill_victim", dummy_test_entry, CapabilityTable::new_for_task(TaskId::new(), 16), 6);
        let pid = sched.get_task(tid).unwrap().process_id.expect("process registered");

        // The budget path only ever looks at the current task.
        sched.current_idx = sched.tasks.iter().position(|t| t.id == tid).unwrap();
        crate::task::process::suspend(tid, 0).expect("suspend the victim's process");
        assert_eq!(
            crate::task::process::record_for_process(pid).unwrap().state,
            crate::task::process::ProcessState::Suspended
        );

        sched.get_task_mut(tid).unwrap().state = TaskState::Running;
        sched.park_current_if_budget_suspended();
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Blocked, "a suspended process parks its current task");

        sched.request_kill(tid, -9).expect("kill the parked task");
        assert_eq!(sched.get_task(tid).unwrap().state, TaskState::Ready);

        // Dispatched again, still inside the suspended process: it must be allowed
        // to run to the syscall-return gate instead of being parked again.
        sched.get_task_mut(tid).unwrap().state = TaskState::Running;
        sched.park_current_if_budget_suspended();
        assert_eq!(
            sched.get_task(tid).unwrap().state,
            TaskState::Running,
            "a pending kill was re-parked by the budget and can never reach its exit gate"
        );

        retire(&mut sched, tid);
    }

    /// The defect this pins: `live_threads_for_process` returned 0 when it lost
    /// the `THREAD_TABLE` race, and `reclaim_task` purges a whole process's futex
    /// waiters on `count <= 1`. A fabricated 0 therefore destroyed the wait state
    /// of siblings that were still running and woke them on a condition that had
    /// not changed.
    ///
    /// WHAT THIS EXERCISES: the count the purge decision is actually made on,
    /// with live siblings present, and the predicate itself.
    ///
    /// WHAT IT CANNOT EXERCISE: the contention window. The count is now read
    /// under a blocking lock, so holding the table and calling it from this
    /// thread would deadlock rather than reproduce the old race. The source pin
    /// below is what forbids the fabricated fallback from returning.
    #[test]
    fn a_process_with_live_siblings_is_not_counted_as_exiting() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let pid = crate::task::ProcessId(0x5E17_0001);
        let siblings = [TaskId(0x5E17_1001), TaskId(0x5E17_1002)];

        let mut placed = 0usize;
        {
            let mut threads = crate::task::process::THREAD_TABLE.lock();
            for slot in threads.iter_mut() {
                if slot.is_none() && placed < siblings.len() {
                    *slot = Some(crate::task::process::ThreadRecord {
                        task: siblings[placed],
                        process: pid,
                    });
                    placed += 1;
                }
            }
        }
        assert_eq!(placed, siblings.len(), "the thread table had no room for this case's siblings");

        let live = crate::task::process::live_threads_for_process(pid);
        assert_eq!(live, siblings.len(), "a live sibling must be counted, not skipped");
        assert!(
            live > 1,
            "reclaim_task purges a process's futex waiters on count <= 1, so this \
             count would destroy the wait state of siblings that are still running"
        );

        {
            let mut threads = crate::task::process::THREAD_TABLE.lock();
            for slot in threads.iter_mut() {
                if slot.as_ref().is_some_and(|r| r.process == pid) {
                    *slot = None;
                }
            }
        }
        assert_eq!(crate::task::process::live_threads_for_process(pid), 0);

        let src = include_str!("process.rs");
        let start = src.find("pub fn live_threads_for_process(")
            .expect("live_threads_for_process must exist for the purge decision to be made");
        let end = src[start..].find("\n}\n").expect("the function body must be closed") + start;
        let body = &src[start..end];
        assert!(
            body.contains("THREAD_TABLE.lock()") && !body.contains("try_lock"),
            "the thread count must be read under a blocking lock: a try_lock \
             fallback returns a 0 that is indistinguishable from 'last thread left'"
        );
    }

    // ------------------------------------------------------------------
    // Deferred reclaim: no terminated task may ever escape it.
    //
    // These cases drive the production path end to end: `terminate_task`
    // queues through `defer_cleanup` exactly as the syscall exit gate does,
    // and `run_deferred_cleanup` is the same drain the kernel main loop and
    // `terminate_current` call. The observable is the kernel-wide delegation
    // registry: only `capability::cleanup_task` inside `reclaim_task`
    // revokes a departed grantee's leases and publishes the revocation, so
    // a lease that stays live after the drain is a reclaim that never ran.
    // ------------------------------------------------------------------

    /// Registers one live delegated lease naming `grantee` as its holder.
    fn live_lease_for(grantee: TaskId) -> u64 {
        let parent = crate::capability::Capability {
            id: crate::capability::CapabilityId(0x0DEF_ACE0_0000_0001),
            cap_type: crate::capability::CapabilityType::SerialOutput,
            rights: crate::capability::Rights::READ | crate::capability::Rights::GRANT,
            object_id: 0x0DEF,
        };
        let lease_id = crate::capability::delegation::allocate_lease_id();
        let lease = crate::capability::delegation::DelegatedLease::new(
            lease_id,
            &parent,
            TaskId(0),
            grantee,
            crate::capability::Rights::READ,
            8,
            100_000,
            crate::interrupts::get_system_ticks(),
        )
        .expect("lease construction");
        assert!(
            crate::capability::delegation::register_lease(lease),
            "the delegation registry had no room for this case's lease"
        );
        lease_id
    }

    /// Whether this case's entries are still awaiting reclaim.
    fn queued_ids(ids: &[TaskId]) -> usize {
        DEFERRED_CLEANUP
            .lock()
            .iter()
            .filter(|entry| ids.contains(&entry.id))
            .count()
    }

    /// More terminations than the old fixed queue could hold must every one
    /// reach reclaim.
    ///
    /// WHY this is the defect and not a leak: the old 64-slot array answered
    /// overflow with a serial warning and dropped the entry, so the 65th and
    /// later departed tasks kept live delegated leases in the kernel-wide
    /// registry -- `has_permission` kept answering for a dead task. The drain
    /// below is the production drain; the leases are the authority that must
    /// not survive it.
    #[test]
    fn every_termination_past_the_old_queue_bound_is_reclaimed() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        // Drain anything an earlier case queued so this case observes only
        // its own terminations.
        run_deferred_cleanup();

        // The old bound was 64; six more exits than it could hold.
        const EXITS: usize = 70;
        let mut victims: Vec<(TaskId, u64)> = Vec::new();
        for _ in 0..EXITS {
            let tid = sched.spawn(
                "reclaim_overflow_probe",
                dummy_test_entry,
                CapabilityTable::new_for_task(TaskId::new(), 16),
                5,
            );
            assert_ne!(tid, TaskId(0), "spawn refused under the process table budget");
            let lease_id = live_lease_for(tid);
            assert!(crate::capability::delegation::is_lease_active(
                lease_id,
                crate::interrupts::get_system_ticks()
            ));
            victims.push((tid, lease_id));
        }
        let ids: Vec<TaskId> = victims.iter().map(|&(tid, _)| tid).collect();

        for &(tid, _) in &victims {
            sched.terminate_task(tid, 0).expect("terminate the probe");
        }
        assert_eq!(
            queued_ids(&ids),
            EXITS,
            "a termination was refused a reclaim slot: the queue dropped an exit"
        );
        // Termination alone revokes the task's own table, never the registry:
        // every lease must still be live here, so the revocations asserted
        // after the drain can only have come from `reclaim_task`.
        for &(_, lease_id) in &victims {
            assert!(
                crate::capability::delegation::is_lease_active(
                    lease_id,
                    crate::interrupts::get_system_ticks()
                ),
                "a lease was revoked before reclaim ran; the observable is no longer isolated"
            );
        }

        run_deferred_cleanup();

        assert_eq!(queued_ids(&ids), 0, "the drain left entries behind");
        for &(tid, lease_id) in &victims {
            assert!(
                crate::capability::capability_revoked(
                    crate::capability::delegation::lease_capability_id(lease_id)
                ),
                "task {} kept live delegated authority after termination: its reclaim was dropped",
                tid.0
            );
        }

        // A second pass is a no-op: nothing is queued and nothing is reclaimed
        // twice.
        run_deferred_cleanup();
        assert_eq!(queued_ids(&ids), 0);

        reap_orphans_until_quiet(&mut sched);
        crate::capability::delegation::prune(crate::interrupts::get_system_ticks());
    }

    /// A task is queued once even if termination is requested twice, and the
    /// drain reclaims it exactly once.
    #[test]
    fn a_termination_is_queued_once_and_reclaimed_once() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        run_deferred_cleanup();

        let tid = sched.spawn(
            "reclaim_dedup_probe",
            dummy_test_entry,
            CapabilityTable::new_for_task(TaskId::new(), 16),
            5,
        );
        assert_ne!(tid, TaskId(0));
        let lease_id = live_lease_for(tid);
        let ids = [tid];

        sched.terminate_task(tid, 0).expect("terminate");
        // A repeated termination is an idempotent no-op, and a repeated defer
        // for the same id must not queue a second reclaim.
        sched.terminate_task(tid, 0).expect("idempotent terminate");
        defer_cleanup(tid, "reclaim_dedup_probe", None);
        assert_eq!(queued_ids(&ids), 1, "one task queued more than one reclaim");

        run_deferred_cleanup();
        assert!(crate::capability::capability_revoked(
            crate::capability::delegation::lease_capability_id(lease_id)
        ));
        assert_eq!(queued_ids(&ids), 0);
        run_deferred_cleanup();
        assert_eq!(queued_ids(&ids), 0, "a reclaimed task was queued again");

        let _ = sched.reap_child(TaskId(0), tid);
        crate::capability::delegation::prune(crate::interrupts::get_system_ticks());
    }

    /// Posts one notification into the global store under `package`/`uid`.
    fn post_global_notification(package: &str, uid: u32) {
        let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
        nm.seed_standard_channels();
        nm.post(
            package,
            uid,
            "system",
            "title",
            "body",
            crate::notification::NotificationPriority::Default,
            crate::notification::LockscreenVisibility::Public,
            1,
            crate::interrupts::get_system_ticks(),
        )
        .expect("the global notification store refused this case's post");
    }

    /// Whether the global store holds a record for `package`/`uid`.
    fn global_notification_present(package: &str, uid: u32) -> bool {
        crate::notification::NOTIFICATION_MANAGER
            .lock()
            .query_active(uid, false)
            .iter()
            .any(|r| r.package_id == package)
    }

    /// Reclaim must cancel the dead task's OWN account's notifications and
    /// never another account's, even when the process record is already gone
    /// by the time the drain runs.
    ///
    /// WHY the record is released before the drain: that is the normal path,
    /// not a contention edge -- `reap_child`/`reap_orphans` release the process
    /// record while the deferred entry is still queued. The old code re-derived
    /// the uid at reclaim time, got None, and `unwrap_or(0)` cancelled user 0's
    /// notifications for the package: a different account's data destroyed,
    /// while the dead task's own records survived.
    #[test]
    fn reclaim_cancels_the_captured_owner_and_never_user_zero() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        run_deferred_cleanup();

        const PKG: &str = "org.interos.reclaimprobe";
        const OTHER_PKG: &str = "org.interos.reclaimprobe.other";
        const OWNER: u32 = 1000;
        {
            let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
            nm.cancel_all_for_package_all_users(PKG);
            nm.cancel_all_for_package_all_users(OTHER_PKG);
        }
        post_global_notification(PKG, 0);
        post_global_notification(PKG, OWNER);
        post_global_notification(OTHER_PKG, OWNER);

        let tid = sched.spawn(
            PKG,
            dummy_test_entry,
            CapabilityTable::new_for_task(TaskId::new(), 16),
            5,
        );
        assert_ne!(tid, TaskId(0));
        crate::task::process::bind_user_for_task(tid, OWNER).expect("bind the owner uid");
        assert_eq!(crate::task::process::user_for_task(tid), Some(OWNER));

        sched.terminate_task(tid, 0).expect("terminate");
        assert_eq!(
            DEFERRED_CLEANUP.lock().iter().find(|e| e.id == tid).map(|e| e.user_id),
            Some(Some(OWNER)),
            "the exit did not capture the owner uid while the process record was live"
        );

        // Release the process record BEFORE reclaim runs, exactly as a parent
        // reap or the orphan reaper does. The owner is now unresolvable.
        sched.reap_child(TaskId(0), tid).expect("reap the child");
        assert!(
            crate::task::process::user_for_task(tid).is_none(),
            "the precondition is broken: the owner is still resolvable at reclaim time"
        );

        run_deferred_cleanup();

        assert!(
            global_notification_present(PKG, 0),
            "user 0's notification for {} was destroyed by a reclaim that could not resolve the owner",
            PKG
        );
        assert!(
            !global_notification_present(PKG, OWNER),
            "the dead task's own notification survived reclaim: the privacy leak the cancellation exists to prevent"
        );
        assert!(
            global_notification_present(OTHER_PKG, OWNER),
            "the cancellation reached beyond the dead task's package"
        );

        let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
        nm.cancel_all_for_package_all_users(PKG);
        nm.cancel_all_for_package_all_users(OTHER_PKG);
    }

    /// When no owner uid can be established at termination, reclaim cancels
    /// nothing and surfaces the failure -- it must not guess uid 0 and must
    /// not fall back to the all-users cancellation.
    #[test]
    fn reclaim_without_an_establishable_owner_cancels_nothing() {
        let _process = crate::task::process::PROCESS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut sched = Scheduler::new();
        sched.init(CapabilityTable::new_for_task(TaskId(0), 16));
        run_deferred_cleanup();

        const PKG: &str = "org.interos.reclaimunknown";
        {
            let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
            nm.cancel_all_for_package_all_users(PKG);
        }
        post_global_notification(PKG, 0);
        post_global_notification(PKG, 1000);

        let tid = sched.spawn(
            PKG,
            dummy_test_entry,
            CapabilityTable::new_for_task(TaskId::new(), 16),
            5,
        );
        assert_ne!(tid, TaskId(0));
        // Release the process record while the task is still registered: at
        // termination no accessor can establish an owner for it.
        crate::task::process::reap(tid).expect("release the process record");
        assert!(crate::task::process::user_for_task(tid).is_none());

        sched.terminate_task(tid, 0).expect("terminate");
        assert_eq!(
            DEFERRED_CLEANUP.lock().iter().find(|e| e.id == tid).map(|e| e.user_id),
            Some(None),
            "an unresolvable owner must be carried as None, never defaulted"
        );

        run_deferred_cleanup();

        assert!(
            global_notification_present(PKG, 0),
            "the unknown-owner fallback destroyed user 0's notifications"
        );
        assert!(
            global_notification_present(PKG, 1000),
            "the unknown-owner fallback destroyed another account's notifications"
        );

        let mut nm = crate::notification::NOTIFICATION_MANAGER.lock();
        nm.cancel_all_for_package_all_users(PKG);
        drop(nm);
        let _ = sched.reap_child(TaskId(0), tid);
    }
}

