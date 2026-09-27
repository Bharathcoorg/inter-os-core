//! InterOS userspace-style system service registry and lifecycle bootstrap.
//!
//! The service registry establishes explicit service ownership and capability boundaries.
//! The service functions are intentionally small kernel-hosted workers until
//! the userspace service ABI is fully available; all privileged work remains
//! behind the existing syscall/capability boundary.

use crate::task::{SCHEDULER, TaskId};
use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceId {
    Init = 0,
    Display = 1,
    Input = 2,
    Storage = 3,
    Network = 4,
    Power = 5,
    Modem = 6,
    Audio = 7,
    Camera = 8,
    Compositor = 9,
    Launcher = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState { Declared, Starting, Running, Stopped, Failed }

#[derive(Debug, Clone, Copy)]
pub struct ServiceRecord { pub id: ServiceId, pub task: TaskId, pub state: ServiceState }

pub static SERVICE_RECORDS: spin::Mutex<[Option<ServiceRecord>; 11]> = spin::Mutex::new([None; 11]);
const HEARTBEAT_TIMEOUT_TICKS: u64 = 500;
const SERVICE_RESTART_BACKOFF_TICKS: u64 = 1000;
static SERVICE_HEARTBEATS: [core::sync::atomic::AtomicU64; 11] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];
static SERVICES_INITIALIZED: AtomicBool = AtomicBool::new(false);
// Serializes lifecycle transitions so start/stop/recovery cannot observe and mutate
// the same service record concurrently. Scheduler locks remain scoped inside this
// boundary; callers never hold the lifecycle lock across context switches.
static SERVICE_LIFECYCLE_LOCK: spin::Mutex<()> = spin::Mutex::new(());
// Restart backoff prevents a failed service from consuming the scheduler in a crash loop.
static SERVICE_RETRY_AFTER: [core::sync::atomic::AtomicU64; 11] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];

/// Registers the system-service set. PID1 owns process creation and lifecycle.
pub fn init() {
    if SERVICES_INITIALIZED.swap(true, Ordering::AcqRel) { return; }
    {
        let mut records = SERVICE_RECORDS.lock();
        records[ServiceId::Init as usize] = Some(ServiceRecord { id: ServiceId::Init, task: TaskId(0), state: ServiceState::Running });
    }
    // Service definitions are registered but not started here. PID 1 owns
    // userspace service lifecycle through the ServiceStart syscall.
    for id in [
        ServiceId::Display, ServiceId::Input, ServiceId::Storage, ServiceId::Network,
        ServiceId::Power, ServiceId::Modem, ServiceId::Audio, ServiceId::Camera,
        ServiceId::Compositor, ServiceId::Launcher,
    ] {
        SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
            id, task: TaskId(0), state: ServiceState::Declared
        });
    }
    crate::serial_println!("[SERVICE] Service definitions registered; PID 1 controls startup.");
}

/// Starts one declared service on behalf of userspace PID 1.
pub fn start(id: ServiceId, parent_id: TaskId) -> Result<TaskId, &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    start_locked(id, parent_id)
}

fn start_locked(id: ServiceId, parent_id: TaskId) -> Result<TaskId, &'static str> {
    if id == ServiceId::Init { return Ok(TaskId(0)); }
    let existing = SERVICE_RECORDS.lock()[id as usize];
    if let Some(record) = existing {
        if record.state == ServiceState::Running && record.task != TaskId(0) { return Ok(record.task); }
        if record.state == ServiceState::Starting { return Err("service already starting"); }
        // A failed userspace service still owns a scheduler task until the explicit
        // recovery path tears it down and reaps it. Do not spawn another
        // process here or recovery could create duplicate service instances.
        if record.state == ServiceState::Failed && record.task != TaskId(0) {
            return Err("failed service requires recovery");
        }
        if !matches!(record.state, ServiceState::Declared | ServiceState::Stopped | ServiceState::Failed) {
            return Err("service is not startable");
        }
    } else { return Err("service not declared"); }
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
        id,
        task: TaskId(0),
        state: ServiceState::Starting,
    });
    let task_result = crate::task::user::spawn_userspace_service(id as u8, parent_id);
    let task = match task_result {
        Ok(task) if task != TaskId(0) => task,
        _ => {
            SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
                id,
                task: TaskId(0),
                state: ServiceState::Failed,
            });
            return Err("service spawn failed");
        }
    };
    crate::ipc::IPC_ROUTER.lock().register_task(task);
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running });
    Ok(task)
}

/// Launches a previously verified signed service package through the authoritative
/// lifecycle registry. This prevents the package-launch syscall from bypassing
/// duplicate-instance, parent-ownership, and recovery state invariants.
pub fn launch_verified_package(
    id: ServiceId,
    parent_id: TaskId,
    manifest: &crate::package::RappManifest,
    payload: &[u8],
) -> Result<TaskId, &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();

    if id == ServiceId::Init {
        return Err("init is not a package-launched service");
    }

    let existing = SERVICE_RECORDS.lock()[id as usize];
    if let Some(record) = existing {
        if record.state == ServiceState::Running && record.task != TaskId(0) {
            let scheduler = SCHEDULER.lock();
            let task = scheduler.get_task(record.task).ok_or("running service task not found")?;
            if task.parent_id != parent_id {
                return Err("running service is not owned by caller");
            }
            return Ok(record.task);
        }
        if record.state == ServiceState::Starting {
            return Err("service already starting");
        }
        if record.state == ServiceState::Failed && record.task != TaskId(0) {
            return Err("failed service requires recovery");
        }
        if !matches!(record.state, ServiceState::Declared | ServiceState::Stopped | ServiceState::Failed) {
            return Err("service is not package-launchable");
        }
    } else {
        return Err("service not declared");
    }

    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
        id,
        task: TaskId(0),
        state: ServiceState::Starting,
    });

    let task = match crate::task::user::spawn_verified_rapp_process(manifest, payload, parent_id) {
        Ok(task) if task != TaskId(0) => task,
        _ => {
            SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
                id,
                task: TaskId(0),
                state: ServiceState::Failed,
            });
            return Err("verified service package spawn failed");
        }
    };

    crate::ipc::IPC_ROUTER.lock().register_task(task);
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
        id,
        task,
        state: ServiceState::Running,
    });
    Ok(task)
}

/// Stops a service task without allowing userspace to terminate arbitrary tasks.
pub fn stop(id: ServiceId, parent_id: TaskId) -> Result<(), &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    if id == ServiceId::Init { return Err("init cannot be stopped"); }
    let record = SERVICE_RECORDS.lock()[id as usize].ok_or("service not declared")?;
    if record.task == TaskId(0) { return Ok(()); }
    // Services are children of the PID1 task that started them. Stopping a
    // service must remain parent-scoped even when the caller has the manager
    // capability; never allow the manager to terminate another owner's task.
    let service_task = record.task;
    {
        let scheduler = SCHEDULER.lock();
        let task = scheduler.get_task(service_task).ok_or("service task not found")?;
        if task.parent_id != parent_id {
            return Err("service is not owned by caller");
        }
    }
    let mut scheduler = SCHEDULER.lock();
    scheduler.terminate_task(service_task, 0).map_err(|_| "service termination failed")?;
    let _ = scheduler.reap_child(parent_id, record.task);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Stopped });
    Ok(())
}


/// Returns a task ID for a named service when registered.
pub fn task_id(id: ServiceId) -> Option<TaskId> {
    SERVICE_RECORDS.lock()[id as usize].map(|r| r.task)
}

/// Records a service heartbeat. Service workers call this before yielding so
/// the supervisor can distinguish a live worker from a dead/stopped task.
pub fn heartbeat(id: ServiceId) {
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
}

pub fn heartbeat_for_task(id: ServiceId, task_id: TaskId) -> Result<(), &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    let record = SERVICE_RECORDS.lock()[id as usize].ok_or("service not declared")?;
    if record.task != task_id || record.state != ServiceState::Running { return Err("service task mismatch"); }
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    Ok(())
}


/// Supervises all registered services and marks missing, terminated, or
/// heartbeat-stale workers as failed. Recovery remains an explicit PID 1 action.
pub fn supervise() {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    let now = crate::interrupts::get_system_ticks();
    // Lock order is scheduler -> registry. Exit paths already follow this
    // order, so supervision must never acquire registry before scheduler.
    let sched = SCHEDULER.lock();
    let mut states = [None; 11];
    let records = *SERVICE_RECORDS.lock();
    for idx in 1..records.len() {
        let Some(record) = records[idx] else { continue; };
        if !matches!(record.state, ServiceState::Running) {
            states[idx] = Some(record.state);
            continue;
        }
        let stale = now.saturating_sub(SERVICE_HEARTBEATS[idx].load(Ordering::Acquire))
            > HEARTBEAT_TIMEOUT_TICKS;
        let failed = match sched.get_task(record.task) {
            Some(task) => matches!(task.state, crate::task::TaskState::Terminated | crate::task::TaskState::Zombie),
            None => true,
        };
        states[idx] = Some(if failed || stale { ServiceState::Failed } else { ServiceState::Running });
    }
    drop(sched);
    let mut registry = SERVICE_RECORDS.lock();
    for idx in 1..registry.len() {
        if let (Some(record), Some(state)) = (records[idx], states[idx]) {
            registry[idx] = Some(ServiceRecord { id: record.id, task: record.task, state });
        }
    }
}
/// Serializes the authoritative service registry for diagnostics.
pub fn snapshot(out: &mut [u8]) -> usize {
    use core::fmt::Write;
    struct W<'a> { b: &'a mut [u8], n: usize }
    impl<'a> Write for W<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let k = s.len().min(self.b.len().saturating_sub(self.n));
            self.b[self.n..self.n + k].copy_from_slice(&s.as_bytes()[..k]);
            self.n += k;
            Ok(())
        }
    }
    let records = SERVICE_RECORDS.lock();
    let mut w = W { b: out, n: 0 };
    for record in records.iter().flatten() {
        let state = match record.state {
            ServiceState::Declared => "declared",
            ServiceState::Starting => "starting",
            ServiceState::Running => "running",
            ServiceState::Stopped => "stopped",
            ServiceState::Failed => "failed",
        };
        let _ = write!(w, "{}|{}|{}\n", record.id as u8, record.task.0, state);
    }
    w.n
}

/// Returns the most recent heartbeat tick for a service.
pub fn last_heartbeat(id: ServiceId) -> u64 {
    SERVICE_HEARTBEATS[id as usize].load(Ordering::Acquire)
}

/// Records a userspace service exit in the authoritative service registry.
pub fn record_exit(task_id: TaskId, _exit_code: i32) {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    let mut records = SERVICE_RECORDS.lock();
    for idx in 1..records.len() {
        if let Some(record) = records[idx] {
            if record.task == task_id {
                records[idx] = Some(ServiceRecord { id: record.id, task: task_id, state: ServiceState::Failed });
                return;
            }
        }
    }
}

/// Restarts failed services when explicitly requested by PID 1.
pub fn restart_failed(parent_id: TaskId) -> usize {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    let mut restarted = 0usize;
    let count = SERVICE_RECORDS.lock().len();
    for idx in 1..count {
        let id = match idx {
            1 => ServiceId::Display, 2 => ServiceId::Input, 3 => ServiceId::Storage,
            4 => ServiceId::Network, 5 => ServiceId::Power, 6 => ServiceId::Modem,
            7 => ServiceId::Audio, 8 => ServiceId::Camera,
            9 => ServiceId::Compositor, 10 => ServiceId::Launcher,
            _ => continue,
        };
        let failed_task = {
            let records = SERVICE_RECORDS.lock();
            match records[idx] {
                Some(ServiceRecord { state: ServiceState::Failed, task, .. }) => Some(task),
                _ => None,
            }
        };
        if let Some(task) = failed_task {
            let now = crate::interrupts::get_system_ticks();
            if now < SERVICE_RETRY_AFTER[idx].load(Ordering::Acquire) { continue; }
            // A heartbeat timeout may leave the worker schedulable. Reserve the
            // retry window before teardown/spawn so repeated spawn failures cannot
            // immediately consume the scheduler in a tight recovery loop.
            SERVICE_RETRY_AFTER[idx].store(now.saturating_add(SERVICE_RESTART_BACKOFF_TICKS), Ordering::Release);
            let mut scheduler = SCHEDULER.lock();
            let _ = scheduler.terminate_task(task, -1);
            let _ = scheduler.reap_child(parent_id, task);
            drop(scheduler);
            SERVICE_RECORDS.lock()[idx] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Declared });
            if start_locked(id, parent_id).is_ok() {
                restarted += 1;
            }
        }
    }
    restarted
}
