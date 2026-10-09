//! Process lifecycle and resource accounting for the InterOS runtime.
//!
//! The scheduler owns execution; this module owns the process-level contract.
//! A process currently has one scheduler thread (the process leader). The data
//! model deliberately separates process identity from TaskId so true multi-thread
//! processes can be added without changing the userspace ABI.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use crate::task::TaskId;

pub const MAX_PROCESSES: usize = 128;
pub const MAX_THREADS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcessState { Running = 0, Background = 1, Suspended = 2, Killed = 3, Restarting = 4, Zombie = 5, Reaped = 6 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_threads: u16,
    pub max_open_files: u16,
    pub max_network_sockets: u16,
    pub max_ipc_endpoints: u16,
    pub max_address_space_bytes: u64,
    pub max_background_cpu_ticks: u64,
    pub max_background_ipc: u16,
    pub max_background_network_bytes: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self { max_threads: 64, max_open_files: 256, max_network_sockets: 16, max_ipc_endpoints: 64, max_address_space_bytes: 256 * 1024 * 1024, max_background_cpu_ticks: 500, max_background_ipc: 64, max_background_network_bytes: 1024 * 1024 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadRecord {
    pub task: TaskId,
    pub process: ProcessId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessRecord {
    pub id: ProcessId,
    pub leader_task: TaskId,
    pub parent_task: TaskId,
    pub state: ProcessState,
    pub exit_code: i32,
    pub thread_count: u16,
    pub open_file_count: u16,
    pub network_socket_count: u16,
    pub ipc_endpoint_count: u16,
    pub address_space_bytes: u64,
    pub limits: ResourceLimits,
    pub user_id: u32,
    pub background_started_ticks: u64,
    pub background_cpu_ticks: u64,
    pub background_ipc: u16,
    pub background_network_bytes: u32,
}

static NEXT_PROCESS_ID: AtomicU64 = AtomicU64::new(1);
pub static PROCESS_TABLE: Mutex<[Option<ProcessRecord>; MAX_PROCESSES]> = Mutex::new([None; MAX_PROCESSES]);
pub static THREAD_TABLE: Mutex<[Option<ThreadRecord>; MAX_THREADS]> = Mutex::new([None; MAX_THREADS]);

pub fn register_leader(task: TaskId, parent_task: TaskId) -> Option<ProcessId> {
    if task == TaskId(0) { return None; }
    let id = ProcessId(NEXT_PROCESS_ID.fetch_add(1, Ordering::Relaxed));
    let record = ProcessRecord { id, leader_task: task, parent_task, state: ProcessState::Running, exit_code: 0, thread_count: 1, open_file_count: 0, network_socket_count: 0, ipc_endpoint_count: 0, address_space_bytes: 0, limits: ResourceLimits::default(), user_id: 0, background_started_ticks: 0, background_cpu_ticks: 0, background_ipc: 0, background_network_bytes: 0 };
    let mut table = PROCESS_TABLE.lock();
    let Some(slot) = table.iter_mut().find(|slot| slot.is_none()) else { return None; };
    let mut threads = THREAD_TABLE.lock();
    let Some(thread_slot) = threads.iter_mut().find(|slot| slot.is_none()) else { return None; };
    *slot = Some(record);
    *thread_slot = Some(ThreadRecord { task, process: id });
    Some(id)
}

pub fn process_for_task(task: TaskId) -> Option<ProcessId> {
    for _ in 0..10_000 {
        if let Some(threads) = THREAD_TABLE.try_lock() {
            return threads.iter().flatten().find(|r| r.task == task).map(|r| r.process);
        }
        core::hint::spin_loop();
    }
    None
}

pub fn bind_user_for_task(task: TaskId, user_id: u32) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    record.user_id = user_id;
    Ok(())
}

pub fn user_for_task(task: TaskId) -> Option<u32> {
    let process = process_for_task(task)?;
    PROCESS_TABLE.lock().iter().flatten().find(|r| r.id == process).map(|r| r.user_id).filter(|uid| *uid != 0)
}

pub fn record_for_process(process: ProcessId) -> Option<ProcessRecord> {
    for _ in 0..10_000 {
        if let Some(table) = PROCESS_TABLE.try_lock() {
            return table.iter().flatten().find(|r| r.id == process).copied();
        }
        core::hint::spin_loop();
    }
    None
}

pub fn set_background(task: TaskId, now: u64) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let r = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if r.state == ProcessState::Running {
        r.state = ProcessState::Background;
        r.background_started_ticks = now;
        r.background_cpu_ticks = 0;
        r.background_ipc = 0;
        r.background_network_bytes = 0;
        Ok(())
    } else {
        Err("process is not foreground")
    }
}

pub fn set_foreground(task: TaskId) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let r = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if r.state == ProcessState::Running {
        return Ok(());
    }
    if matches!(r.state, ProcessState::Background | ProcessState::Suspended) {
        r.state = ProcessState::Running;
        r.background_started_ticks = 0;
        r.background_cpu_ticks = 0;
        r.background_ipc = 0;
        r.background_network_bytes = 0;
        Ok(())
    } else { Err("process is not backgrounded") }
}

pub fn suspend(task: TaskId, now: u64) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let r = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if matches!(r.state, ProcessState::Running | ProcessState::Background) {
        r.state = ProcessState::Suspended;
        r.background_started_ticks = now;
        r.background_cpu_ticks = 0;
        drop(table);
        if let Some(mut sched) = crate::task::SCHEDULER.try_lock() {
            sched.block_task(task);
        }
        Ok(())
    } else { Err("process cannot be suspended") }
}

pub fn wake(task: TaskId) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let r = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if r.state == ProcessState::Suspended {
        r.state = ProcessState::Background;
        r.background_started_ticks = crate::interrupts::get_system_ticks();
        r.background_cpu_ticks = 0;
        drop(table);
        if let Some(mut sched) = crate::task::SCHEDULER.try_lock() {
            sched.unblock_task(task);
        }
        Ok(())
    } else { Err("process is not suspended") }
}

pub fn wake_package(package_id: &str, user_id: u32) -> usize {
    let mut candidates = [TaskId(0); MAX_PROCESSES];
    let mut candidate_count = 0usize;
    {
        let table = PROCESS_TABLE.lock();
        let threads = THREAD_TABLE.lock();
        for r in table.iter().flatten() {
            if r.user_id != user_id || !matches!(r.state, ProcessState::Suspended | ProcessState::Background) { continue; }
            if let Some(thread) = threads.iter().flatten().find(|t| t.process == r.id) {
                if candidate_count < candidates.len() {
                    candidates[candidate_count] = thread.task;
                    candidate_count += 1;
                }
            }
        }
    }

    let mut count = 0usize;
    for task in candidates[..candidate_count].iter().copied() {
        let is_target = crate::task::SCHEDULER.lock()
            .get_task(task)
            .map(|t| t.name == package_id)
            .unwrap_or(false);
        if !is_target { continue; }

        let process = match process_for_task(task) { Some(p) => p, None => continue };
        let woke = {
            let mut table = PROCESS_TABLE.lock();
            if let Some(r) = table.iter_mut().flatten().find(|r| r.id == process) {
                if r.user_id == user_id && r.state == ProcessState::Suspended {
                    r.state = ProcessState::Background;
                    r.background_started_ticks = crate::interrupts::get_system_ticks();
                    true
                } else { false }
            } else { false }
        };
        if woke {
            crate::task::SCHEDULER.lock().unblock_task(task);
            count += 1;
        }
    }
    count
}

pub fn consume_background_ipc(task: TaskId) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let r = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if r.state != ProcessState::Background { return Ok(()); }
    if r.background_ipc >= r.limits.max_background_ipc { return Err("background IPC quota exceeded"); }
    r.background_ipc += 1;
    Ok(())
}

pub fn consume_background_network(task: TaskId, bytes: usize) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("process not found")?;
    let mut table = PROCESS_TABLE.lock();
    let r = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if r.state != ProcessState::Background { return Ok(()); }
    let next = r.background_network_bytes.saturating_add(bytes as u32);
    if next > r.limits.max_background_network_bytes { return Err("background network quota exceeded"); }
    r.background_network_bytes = next;
    Ok(())
}

pub fn account_background_cpu_tick(task: TaskId) {
    let Some(process) = process_for_task(task) else { return; };
    if let Some(mut table) = PROCESS_TABLE.try_lock() {
        if let Some(r) = table.iter_mut().flatten().find(|r| r.id == process) {
            if r.state == ProcessState::Background {
                r.background_cpu_ticks = r.background_cpu_ticks.saturating_add(1);
            }
        }
    }
}

pub fn enforce_background_budget() -> usize {
    if let Some(mut table) = PROCESS_TABLE.try_lock() {
        let mut suspended = 0;
        for r in table.iter_mut().flatten() {
            if r.state == ProcessState::Background
                && r.background_cpu_ticks >= r.limits.max_background_cpu_ticks
            {
                r.state = ProcessState::Suspended;
                suspended += 1;
            }
        }
        suspended
    } else {
        0
    }
}

pub fn can_reserve_ipc_endpoint(process: ProcessId) -> Result<(), &'static str> {
    let table = PROCESS_TABLE.lock();
    let record = table.iter().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if !matches!(record.state, ProcessState::Running | ProcessState::Background) { return Err("process is not runnable"); }
    if record.ipc_endpoint_count >= record.limits.max_ipc_endpoints { return Err("IPC endpoint limit exceeded"); }
    Ok(())
}

pub fn reserve_ipc_endpoint(process: ProcessId) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if !matches!(record.state, ProcessState::Running | ProcessState::Background) { return Err("process is not runnable"); }
    if record.ipc_endpoint_count >= record.limits.max_ipc_endpoints { return Err("IPC endpoint limit exceeded"); }
    record.ipc_endpoint_count += 1;
    Ok(())
}

pub fn release_ipc_endpoint(process: ProcessId) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(record) = table.iter_mut().flatten().find(|r| r.id == process) {
        record.ipc_endpoint_count = record.ipc_endpoint_count.saturating_sub(1);
    }
}

pub fn reserve_network_socket(process: ProcessId) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if !matches!(record.state, ProcessState::Running | ProcessState::Background) { return Err("process is not runnable"); }
    if record.network_socket_count >= record.limits.max_network_sockets { return Err("network socket limit exceeded"); }
    record.network_socket_count += 1;
    Ok(())
}

pub fn release_network_socket(process: ProcessId) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(record) = table.iter_mut().flatten().find(|r| r.id == process) {
        record.network_socket_count = record.network_socket_count.saturating_sub(1);
    }
}

pub fn can_open_file(process: ProcessId) -> Result<(), &'static str> {
    let table = PROCESS_TABLE.lock();
    let record = table.iter().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if !matches!(record.state, ProcessState::Running | ProcessState::Background) { return Err("process is not runnable"); }
    if record.open_file_count >= record.limits.max_open_files { return Err("open file limit exceeded"); }
    Ok(())
}

pub fn reserve_open_file(process: ProcessId) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if !matches!(record.state, ProcessState::Running | ProcessState::Background) { return Err("process is not runnable"); }
    if record.open_file_count >= record.limits.max_open_files { return Err("open file limit exceeded"); }
    record.open_file_count += 1;
    Ok(())
}

pub fn release_open_file(process: ProcessId) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(record) = table.iter_mut().flatten().find(|r| r.id == process) {
        record.open_file_count = record.open_file_count.saturating_sub(1);
    }
}

pub fn set_address_space_usage(process: ProcessId, bytes: usize) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if bytes as u64 > record.limits.max_address_space_bytes {
        return Err("address-space limit exceeded");
    }
    record.address_space_bytes = bytes as u64;
    Ok(())
}

pub fn reserve_address_space(process: ProcessId, bytes: usize) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if record.state != ProcessState::Running { return Err("process is not running"); }
    let next = record.address_space_bytes.checked_add(bytes as u64).ok_or("address-space counter overflow")?;
    if next > record.limits.max_address_space_bytes { return Err("address-space limit exceeded"); }
    record.address_space_bytes = next;
    Ok(())
}

pub fn release_address_space(process: ProcessId, bytes: usize) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(record) = table.iter_mut().flatten().find(|r| r.id == process) {
        record.address_space_bytes = record.address_space_bytes.saturating_sub(bytes as u64);
    }
}

pub fn can_create_thread(process: ProcessId) -> Result<(), &'static str> {
    let table = PROCESS_TABLE.lock();
    let record = table.iter().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if record.state != ProcessState::Running { return Err("process is not running"); }
    if record.thread_count >= record.limits.max_threads { return Err("thread limit exceeded"); }
    Ok(())
}

pub fn reserve_thread(process: ProcessId) -> Result<(), &'static str> {
    account_thread(process)
}

pub fn cancel_thread_reservation(process: ProcessId) {
    release_thread(process)
}

pub fn bind_reserved_thread(process: ProcessId, task: TaskId) -> Result<(), &'static str> {
    if task == TaskId(0) { return Err("invalid thread task"); }
    let mut threads = THREAD_TABLE.lock();
    if let Some(slot) = threads.iter_mut().find(|slot| slot.is_none()) {
        *slot = Some(ThreadRecord { task, process });
        Ok(())
    } else {
        // Reservation ownership stays with the caller on failure. The scheduler
        // decides whether to cancel it after all task-local rollback is complete.
        Err("thread table full")
    }
}

pub fn register_thread(process: ProcessId, task: TaskId) -> Result<(), &'static str> {
    reserve_thread(process)?;
    if let Err(error) = bind_reserved_thread(process, task) {
        // The reservation is owned by this transactional helper. Do not leave
        // thread accounting elevated when the identity binding cannot commit.
        cancel_thread_reservation(process);
        return Err(error);
    }
    Ok(())
}

pub fn thread_started(process: ProcessId) -> Result<(), &'static str> { account_thread(process) }

pub fn mark_zombie(task: TaskId, exit_code: i32) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(record) = table.iter_mut().flatten().find(|r| r.leader_task == task) {
        record.state = ProcessState::Zombie;
        record.exit_code = exit_code;
    }
}

pub fn reap(task: TaskId) -> Result<(), &'static str> {
    let process = process_for_task(task).ok_or("thread not registered")?;
    let is_leader = record_for_process(process).map(|r| r.leader_task == task).unwrap_or(false);

    let mut threads = THREAD_TABLE.lock();
    if let Some(slot) = threads.iter_mut().find(|slot| slot.as_ref().map(|r| r.task == task).unwrap_or(false)) {
        *slot = None;
    } else {
        return Err("thread not registered");
    }
    let remaining = threads.iter().flatten().filter(|r| r.process == process).count();
    drop(threads);

    let mut table = PROCESS_TABLE.lock();
    let Some(slot) = table.iter_mut().find(|slot| slot.as_ref().map(|r| r.id == process).unwrap_or(false)) else {
        return Err("process record missing");
    };

    if is_leader {
        if remaining == 0 {
            *slot = None;
        } else if let Some(record) = slot.as_mut() {
            // A process leader may be reaped before sibling threads finish.
            // Preserve the zombie record for lifecycle accounting while clearing
            // the stale task identity that no longer has a thread-table binding.
            record.leader_task = TaskId(0);
            record.state = ProcessState::Zombie;
        }
        return Ok(());
    }

    if let Some(record) = slot.as_mut() {
        record.thread_count = record.thread_count.saturating_sub(1).max(1);
        if remaining == 0 && record.state == ProcessState::Zombie {
            *slot = None;
        }
    }
    Ok(())
}

pub fn is_leader(task: TaskId) -> bool {
    process_for_task(task).and_then(record_for_process).map(|r| r.leader_task == task).unwrap_or(false)
}

pub fn set_limits(process: ProcessId, limits: ResourceLimits) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if limits.max_threads == 0 || limits.max_open_files == 0 || limits.max_network_sockets == 0 || limits.max_ipc_endpoints == 0 || limits.max_address_space_bytes < 4096 { return Err("invalid resource limits"); }
    if record.thread_count > limits.max_threads { return Err("limits below current usage"); }
    if record.open_file_count as u16 > limits.max_open_files { return Err("limits below current open-file usage"); }
    if record.ipc_endpoint_count > limits.max_ipc_endpoints { return Err("limits below current IPC endpoint usage"); }
    if record.address_space_bytes > limits.max_address_space_bytes { return Err("limits below current address-space usage"); }
    record.limits = limits;
    Ok(())
}

pub fn account_thread(process: ProcessId) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if record.state != ProcessState::Running { return Err("process is not running"); }
    if record.thread_count >= record.limits.max_threads { return Err("thread limit exceeded"); }
    record.thread_count += 1;
    Ok(())
}

pub fn release_thread(process: ProcessId) {
    let mut table = PROCESS_TABLE.lock();
    if let Some(record) = table.iter_mut().flatten().find(|r| r.id == process) { record.thread_count = record.thread_count.saturating_sub(1).max(1); }
}

pub fn validate() -> bool {
    let table = PROCESS_TABLE.lock();
    let threads = THREAD_TABLE.lock();
    let mut seen = [TaskId(0); MAX_PROCESSES];
    let mut count = 0;
    for record in table.iter().flatten() {
        if record.thread_count == 0 || record.thread_count > record.limits.max_threads { return false; }
        if record.open_file_count > record.limits.max_open_files { return false; }
        if record.network_socket_count > record.limits.max_network_sockets { return false; }
        if record.ipc_endpoint_count > record.limits.max_ipc_endpoints { return false; }
        if record.address_space_bytes > record.limits.max_address_space_bytes { return false; }
        let actual_threads = threads.iter().flatten().filter(|t| t.process == record.id).count();
        if actual_threads != record.thread_count as usize { return false; }
        if record.leader_task != TaskId(0) {
            let leader_binding = threads.iter().flatten()
                .any(|t| t.task == record.leader_task && t.process == record.id);
            if !leader_binding { return false; }
            if seen[..count].contains(&record.leader_task) { return false; }
            seen[count] = record.leader_task;
            count += 1;
        } else if record.state != ProcessState::Zombie {
            return false;
        }
    }

    for thread in threads.iter().flatten() {
        if !table.iter().flatten().any(|record| record.id == thread.process) {
            return false;
        }
    }
    true
}

pub fn snapshot(out: &mut [u8]) -> usize {
    use core::fmt::Write;
    struct W<'a> { b: &'a mut [u8], n: usize }
    impl<'a> Write for W<'a> { fn write_str(&mut self, s: &str) -> core::fmt::Result { let k=s.len().min(self.b.len().saturating_sub(self.n)); self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]); self.n+=k; Ok(()) } }
    let table=PROCESS_TABLE.lock(); let mut w=W{b:out,n:0};
    for r in table.iter().flatten() { let _=writeln!(w,"{}|{}|{}|{}|{}|{}|{}|{}\n",r.id.0,r.leader_task.0,r.parent_task.0,r.state as u8,r.thread_count,r.open_file_count,r.ipc_endpoint_count,r.address_space_bytes); }
    w.n
}
