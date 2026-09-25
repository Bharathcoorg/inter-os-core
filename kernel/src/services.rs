//! InterOS userspace-style system service registry and lifecycle bootstrap.
//!
//! Phase 25 establishes explicit service ownership and capability boundaries.
//! The service functions are intentionally small kernel-hosted workers until
//! the userspace service ABI is fully available; all privileged work remains
//! behind the existing syscall/capability boundary.

use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::task::{SCHEDULER, TaskId};
use crate::syscall;
use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceId { Init=0, Display=1, Input=2, Storage=3, Network=4, Power=5, Modem=6, Audio=7, Camera=8 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState { Declared, Starting, Running, Stopped, Failed }

#[derive(Debug, Clone, Copy)]
pub struct ServiceRecord { pub id: ServiceId, pub task: TaskId, pub state: ServiceState }

fn caps(id: ServiceId) -> CapabilityTable {
    let mut t = CapabilityTable::new(16);
    let base = 0x5000 + id as u64 * 0x10;
    let _ = t.insert(Capability { id: CapabilityId(base), cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 });
    // IpcSend is intentionally omitted here. init() installs only explicit
    // sender -> receiver object capabilities after all service task IDs exist.
    let _ = t.insert(Capability { id: CapabilityId(base + 2), cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 });
    let (kind, rights) = match id {
        ServiceId::Display => (CapabilityType::FrameBufferDirect, Rights::WRITE),
        ServiceId::Input => (CapabilityType::EventPoll, Rights::READ),
        ServiceId::Storage => (CapabilityType::FileSystemRead, Rights::READ | Rights::WRITE),
        ServiceId::Network => (CapabilityType::NetworkSocket, Rights::READ | Rights::WRITE),
        ServiceId::Power | ServiceId::Modem | ServiceId::Audio | ServiceId::Camera => (CapabilityType::SystemControl, Rights::INVOKE),
        ServiceId::Init => (CapabilityType::ProcessSpawn, Rights::GRANT),
    };
    let _ = t.insert(Capability { id: CapabilityId(base + 3), cap_type: kind, rights, object_id: id as u64 });
    let _ = t.insert(Capability { id: CapabilityId(base + 4), cap_type: CapabilityType::SerialOutput, rights: Rights::WRITE, object_id: 0 });
    t
}

extern "C" fn service_loop(id: ServiceId, name: &'static str) -> ! {
    syscall::sys_log(4, name);
    loop { heartbeat(id); syscall::sys_yield(); }
}

extern "C" fn displayd() -> ! { service_loop(ServiceId::Display, "[displayd] Display service online: compositor/DRM boundary active.") }
extern "C" fn inputd() -> ! { service_loop(ServiceId::Input, "[inputd] Input service online: touch/gesture boundary active.") }
extern "C" fn storaged() -> ! { service_loop(ServiceId::Storage, "[storaged] Storage service online: VFS/block boundary active.") }
extern "C" fn netd() -> ! { service_loop(ServiceId::Network, "[netd] Network service online: socket/network HAL boundary active.") }
extern "C" fn powerd() -> ! { service_loop(ServiceId::Power, "[powerd] Power service online: PMIC/DVFS boundary active.") }
extern "C" fn modemd() -> ! { service_loop(ServiceId::Modem, "[modemd] Modem service online: cellular/RIL boundary active.") }
extern "C" fn audiod() -> ! { service_loop(ServiceId::Audio, "[audiod] Audio service online: codec/audio HAL boundary active.") }
extern "C" fn camerad() -> ! { service_loop(ServiceId::Camera, "[camerad] Camera service online: sensor/CSI boundary active.") }

pub static SERVICE_RECORDS: spin::Mutex<[Option<ServiceRecord>; 9]> = spin::Mutex::new([None; 9]);
const HEARTBEAT_TIMEOUT_TICKS: u64 = 500;
const PID1: TaskId = TaskId(0);
static SERVICE_HEARTBEATS: [core::sync::atomic::AtomicU64; 9] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];
static SERVICES_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Starts the Phase 25 service set once, with isolated capability tables.
pub fn init() {
    if SERVICES_INITIALIZED.swap(true, Ordering::AcqRel) { return; }
    {
        let mut records = SERVICE_RECORDS.lock();
        records[ServiceId::Init as usize] = Some(ServiceRecord { id: ServiceId::Init, task: TaskId(0), state: ServiceState::Running });
    }
    // Service definitions are registered but not started here. PID 1 owns
    // userspace service lifecycle through the ServiceStart syscall.
    let defs: [(ServiceId, &'static str, extern "C" fn() -> !, u8); 8] = [
        (ServiceId::Display, "displayd", displayd, 6),
        (ServiceId::Input, "inputd", inputd, 6),
        (ServiceId::Storage, "storaged", storaged, 5),
        (ServiceId::Network, "netd", netd, 5),
        (ServiceId::Power, "powerd", powerd, 7),
        (ServiceId::Modem, "modemd", modemd, 6),
        (ServiceId::Audio, "audiod", audiod, 5),
        (ServiceId::Camera, "camerad", camerad, 5),
    ];
    for (id, _name, _entry, _priority) in defs {
        SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
            id, task: TaskId(0), state: ServiceState::Declared
        });
    }
    crate::serial_println!("[SERVICE] Service definitions registered; PID 1 controls startup.");
}
}

/// Starts one declared service on behalf of userspace PID 1.
pub fn start(id: ServiceId) -> Result<TaskId, &'static str> {
    if id == ServiceId::Init { return Ok(TaskId(0)); }
    let existing = SERVICE_RECORDS.lock()[id as usize];
    if let Some(record) = existing {
        if record.state == ServiceState::Running && record.task != TaskId(0) { return Ok(record.task); }
    } else { return Err("service not declared"); }
    let (name, entry, priority): (&'static str, extern "C" fn() -> !, u8) = match id {
        ServiceId::Display => ("displayd", displayd, 6), ServiceId::Input => ("inputd", inputd, 6),
        ServiceId::Storage => ("storaged", storaged, 5), ServiceId::Network => ("netd", netd, 5),
        ServiceId::Power => ("powerd", powerd, 7), ServiceId::Modem => ("modemd", modemd, 6),
        ServiceId::Audio => ("audiod", audiod, 5), ServiceId::Camera => ("camerad", camerad, 5),
        ServiceId::Init => return Ok(TaskId(0)),
    };
    let task = SCHEDULER.lock().spawn_with_parent(name, entry, caps(id), priority, PID1);
    crate::ipc::IPC_ROUTER.lock().register_task(task);
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running });
    Ok(task)
}

/// Stops a service task without allowing userspace to terminate arbitrary tasks.
pub fn stop(id: ServiceId) -> Result<(), &'static str> {
    if id == ServiceId::Init { return Err("init cannot be stopped"); }
    let record = SERVICE_RECORDS.lock()[id as usize].ok_or("service not declared")?;
    if record.task == TaskId(0) { return Ok(()); }
    SCHEDULER.lock().terminate_task(record.task, 0).map_err(|_| "service termination failed")?;
    let _ = SCHEDULER.lock().reap_child(PID1, record.task);
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

/// Supervises all registered services and marks missing/terminated workers as
/// failed. Restart orchestration is intentionally deferred until a userspace
/// init/service-manager ABI exists; the kernel never silently recreates a task.
pub fn supervise() {
    let now = crate::interrupts::get_system_ticks();
    let mut records = SERVICE_RECORDS.lock();
    let sched = SCHEDULER.lock();
    for idx in 1..records.len() {
        let id = match idx {
            1 => ServiceId::Display, 2 => ServiceId::Input, 3 => ServiceId::Storage,
            4 => ServiceId::Network, 5 => ServiceId::Power, 6 => ServiceId::Modem,
            7 => ServiceId::Audio, 8 => ServiceId::Camera, _ => continue,
        };
        let Some(record) = records[idx] else { continue; };
        let task_id = record.task;
        let stale = now.saturating_sub(SERVICE_HEARTBEATS[idx].load(Ordering::Acquire)) > HEARTBEAT_TIMEOUT_TICKS;
        let failed = match sched.get_task(task_id) {
            Some(task) => matches!(task.state, crate::task::TaskState::Terminated | crate::task::TaskState::Zombie),
            None => true,
        };
        let state = match record.state {
            ServiceState::Stopped => ServiceState::Stopped,
            ServiceState::Declared | ServiceState::Starting => record.state,
            ServiceState::Running if failed || stale => ServiceState::Failed,
            ServiceState::Running => ServiceState::Running,
            ServiceState::Failed => ServiceState::Failed,
        };
        records[idx] = Some(ServiceRecord { id, task: task_id, state });
    }
}

/// Serializes the authoritative Phase 25 service registry for diagnostics.
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

/// Records a worker exit in the authoritative hardware-service registry.
pub fn record_exit(task_id: TaskId, _exit_code: i32) {
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
pub fn restart_failed() -> usize {
    let mut restarted = 0usize;
    for idx in 1..SERVICE_RECORDS.lock().len() {
        let id = match idx {
            1 => ServiceId::Display, 2 => ServiceId::Input, 3 => ServiceId::Storage,
            4 => ServiceId::Network, 5 => ServiceId::Power, 6 => ServiceId::Modem,
            7 => ServiceId::Audio, 8 => ServiceId::Camera, _ => continue,
        };
        let failed_task = match SERVICE_RECORDS.lock()[idx] {
            Some(ServiceRecord { state: ServiceState::Failed, task }) => Some(task),
            _ => None,
        };
        if let Some(task) = failed_task {
            let _ = SCHEDULER.lock().reap_child(PID1, task);
            if start(id).is_ok() { restarted += 1; }
        }
    }
    restarted
}
