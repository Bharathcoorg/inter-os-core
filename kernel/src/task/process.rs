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
pub enum ProcessState { Running = 0, Zombie = 1, Reaped = 2 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_threads: u16,
    pub max_open_files: u16,
    pub max_ipc_endpoints: u16,
    pub max_address_space_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self { max_threads: 64, max_open_files: 256, max_ipc_endpoints: 64, max_address_space_bytes: 256 * 1024 * 1024 }
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
    pub limits: ResourceLimits,
}

static NEXT_PROCESS_ID: AtomicU64 = AtomicU64::new(1);
pub static PROCESS_TABLE: Mutex<[Option<ProcessRecord>; MAX_PROCESSES]> = Mutex::new([None; MAX_PROCESSES]);
pub static THREAD_TABLE: Mutex<[Option<ThreadRecord>; MAX_THREADS]> = Mutex::new([None; MAX_THREADS]);

pub fn register_leader(task: TaskId, parent_task: TaskId) -> Option<ProcessId> {
    if task == TaskId(0) { return None; }
    let id = ProcessId(NEXT_PROCESS_ID.fetch_add(1, Ordering::Relaxed));
    let record = ProcessRecord { id, leader_task: task, parent_task, state: ProcessState::Running, exit_code: 0, thread_count: 1, limits: ResourceLimits::default() };
    let mut table = PROCESS_TABLE.lock();
    let Some(slot) = table.iter_mut().find(|slot| slot.is_none()) else { return None; };
    let mut threads = THREAD_TABLE.lock();
    let Some(thread_slot) = threads.iter_mut().find(|slot| slot.is_none()) else { return None; };
    *slot = Some(record);
    *thread_slot = Some(ThreadRecord { task, process: id });
    Some(id)
}

pub fn process_for_task(task: TaskId) -> Option<ProcessId> {
    THREAD_TABLE.lock().iter().flatten().find(|r| r.task == task).map(|r| r.process)
}

pub fn record_for_process(process: ProcessId) -> Option<ProcessRecord> {
    PROCESS_TABLE.lock().iter().flatten().find(|r| r.id == process).copied()
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
        release_thread(process);
        Err("thread table full")
    }
}

pub fn register_thread(process: ProcessId, task: TaskId) -> Result<(), &'static str> {
    reserve_thread(process)?;
    bind_reserved_thread(process, task)
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
    if is_leader {
        if remaining != 0 { return Err("process still has live threads"); }
        let mut table = PROCESS_TABLE.lock();
        if let Some(slot) = table.iter_mut().find(|slot| slot.as_ref().map(|r| r.id == process).unwrap_or(false)) {
            *slot = None;
            return Ok(());
        }
        Err("process record missing")
    } else {
        release_thread(process);
        Ok(())
    }
}

pub fn is_leader(task: TaskId) -> bool {
    process_for_task(task).and_then(record_for_process).map(|r| r.leader_task == task).unwrap_or(false)
}

pub fn set_limits(process: ProcessId, limits: ResourceLimits) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let record = table.iter_mut().flatten().find(|r| r.id == process).ok_or("process not found")?;
    if limits.max_threads == 0 || limits.max_open_files == 0 || limits.max_ipc_endpoints == 0 || limits.max_address_space_bytes < 4096 { return Err("invalid resource limits"); }
    if record.thread_count > limits.max_threads { return Err("limits below current usage"); }
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
    let mut seen = [TaskId(0); MAX_PROCESSES];
    let mut count = 0;
    for record in table.iter().flatten() {
        if record.leader_task == TaskId(0) || record.thread_count == 0 || record.thread_count > record.limits.max_threads { return false; }
        let leader_binding = THREAD_TABLE.lock().iter().flatten().any(|t| t.task == record.leader_task && t.process == record.id);
        if !leader_binding { return false; }
        if seen[..count].contains(&record.leader_task) { return false; }
        seen[count] = record.leader_task;
        count += 1;
    }
    true
}

pub fn snapshot(out: &mut [u8]) -> usize {
    use core::fmt::Write;
    struct W<'a> { b: &'a mut [u8], n: usize }
    impl<'a> Write for W<'a> { fn write_str(&mut self, s: &str) -> core::fmt::Result { let k=s.len().min(self.b.len().saturating_sub(self.n)); self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]); self.n+=k; Ok(()) } }
    let table=PROCESS_TABLE.lock(); let mut w=W{b:out,n:0};
    for r in table.iter().flatten() { let _=writeln!(w,"{}|{}|{}|{}|{}\n",r.id.0,r.leader_task.0,r.parent_task.0,r.state as u8,r.thread_count); }
    w.n
}
