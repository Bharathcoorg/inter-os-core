//! InterOS userspace-style system service registry and lifecycle bootstrap.
//!
//! The service registry establishes explicit service ownership and capability boundaries.
//! The kernel owns only the authoritative lifecycle registry and enforcement boundary;
//! PID 1 owns service startup/recovery policy and userspace service execution.

use alloc::vec::Vec;
use crate::capability::{CapabilityType, Rights};
use crate::task::{SCHEDULER, TaskId, ResourceLimits};
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
    Sensor = 11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState { Declared, Starting, Running, Stopped, Failed }

pub const SERVICE_ABI_VERSION: u16 = 1;
pub const HEARTBEAT_TIMEOUT_TICKS: u64 = 500;
pub const SERVICE_START_TIMEOUT_TICKS: u64 = 500;
pub const SERVICE_POWER_CONTROL_OBJECT: u64 = 0x504F_5745_5200_01;
pub const SERVICE_TELEPHONY_CONTROL_OBJECT: u64 = 0x5445_4C45_0000_01;
pub const SERVICE_AUDIO_CONTROL_OBJECT: u64 = 0x4155_4449_0000_01;
pub const SERVICE_CAMERA_CONTROL_OBJECT: u64 = 0x4341_4D45_0000_01;
pub const SERVICE_SENSOR_CONTROL_OBJECT: u64 = 0x5345_4E53_0000_01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy { Manual, OnFailure }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceDescriptor {
    pub id: ServiceId,
    pub abi_version: u16,
    pub restart: RestartPolicy,
    pub critical: bool,
    pub dependencies: &'static [ServiceId],
}

const NO_DEPS: &[ServiceId] = &[];
const POWER_DEP: &[ServiceId] = &[ServiceId::Power];
const UI_DEPS: &[ServiceId] = &[ServiceId::Display, ServiceId::Input];
const LAUNCHER_DEPS: &[ServiceId] = &[ServiceId::Compositor];

pub fn descriptor(id: ServiceId) -> ServiceDescriptor {
    match id {
        ServiceId::Init => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::Manual, critical: true, dependencies: NO_DEPS },
        ServiceId::Display | ServiceId::Input | ServiceId::Storage => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: true, dependencies: NO_DEPS },
        ServiceId::Network | ServiceId::Modem => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: false, dependencies: NO_DEPS },
        ServiceId::Power => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: true, dependencies: NO_DEPS },
        ServiceId::Audio | ServiceId::Camera => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: false, dependencies: POWER_DEP },
        ServiceId::Compositor => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: true, dependencies: UI_DEPS },
        ServiceId::Launcher => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: true, dependencies: LAUNCHER_DEPS },
        ServiceId::Sensor => ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: false, dependencies: &[ServiceId::Power] },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCapabilitySpec {
    pub cap_type: CapabilityType,
    pub rights: Rights,
    pub object_id: u64,
}

pub fn capability_policy(id: ServiceId, parent_id: TaskId) -> Result<Vec<ServiceCapabilitySpec>, &'static str> {
    let mut policy = Vec::with_capacity(10);
    policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SerialOutput, rights: Rights::WRITE, object_id: 0 });
    match id {
        ServiceId::Compositor => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FrameBufferDirect, rights: Rights::READ | Rights::WRITE, object_id: 1 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
        }
        ServiceId::Launcher => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: parent_id.0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FrameBufferDirect, rights: Rights::READ | Rights::WRITE, object_id: 1 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::ProcessSpawn, rights: Rights::INVOKE, object_id: 0 });
        }
        ServiceId::Display => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FrameBufferDirect, rights: Rights::WRITE, object_id: 1 }),
        ServiceId::Storage => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemWrite, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::StorageRawBlock, rights: Rights::READ, object_id: 0 });
        }
        ServiceId::Network => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::NetworkSocket, rights: Rights::READ | Rights::WRITE, object_id: ServiceId::Network as u64 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::NetworkSend, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::NetworkReceive, rights: Rights::READ, object_id: 0 });
        }
        ServiceId::Power => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_POWER_CONTROL_OBJECT }),
        ServiceId::Modem => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_TELEPHONY_CONTROL_OBJECT }),
        ServiceId::Audio => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_AUDIO_CONTROL_OBJECT }),
        ServiceId::Camera => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_CAMERA_CONTROL_OBJECT }),
        ServiceId::Sensor => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_SENSOR_CONTROL_OBJECT }),
        ServiceId::Input => {}
        ServiceId::Init => return Err("init does not use userspace service capability policy"),
    }
    if !matches!(id, ServiceId::Compositor | ServiceId::Launcher) {
        policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 });
        policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
        policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 });
    }
    Ok(policy)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceInfo { pub id: ServiceId, pub abi_version: u16, pub state: ServiceState, pub restart: RestartPolicy, pub critical: bool, pub generation: u32 }

#[derive(Debug, Clone, Copy)]
pub struct ServiceRecord { pub id: ServiceId, pub task: TaskId, pub state: ServiceState, pub generation: u32 }

pub static SERVICE_RECORDS: spin::Mutex<[Option<ServiceRecord>; 12]> = spin::Mutex::new([None; 12]);
const SERVICE_RESTART_BACKOFF_TICKS: u64 = 1000;
static SERVICE_HEARTBEATS: [core::sync::atomic::AtomicU64; 12] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];
static SERVICE_FAILURES: [core::sync::atomic::AtomicU64; 12] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];
static SERVICES_INITIALIZED: AtomicBool = AtomicBool::new(false);
// Serializes lifecycle transitions so start/stop/recovery cannot observe and mutate
// the same service record concurrently. Scheduler locks remain scoped inside this
// boundary; callers never hold the lifecycle lock across context switches.
static SERVICE_LIFECYCLE_LOCK: spin::Mutex<()> = spin::Mutex::new(());
// Restart backoff prevents a failed service from consuming the scheduler in a crash loop.
static SERVICE_STARTED_AT: [core::sync::atomic::AtomicU64; 12] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];
static SERVICE_RETRY_AFTER: [core::sync::atomic::AtomicU64; 12] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
];

/// Registers the system-service set. PID1 owns process creation and lifecycle.
pub fn init() {
    if SERVICES_INITIALIZED.swap(true, Ordering::AcqRel) { return; }
    {
        let mut records = SERVICE_RECORDS.lock();
        records[ServiceId::Init as usize] = Some(ServiceRecord { id: ServiceId::Init, task: TaskId(0), state: ServiceState::Running, generation: 1 });
    }
    // Service definitions are registered but not started here. PID 1 owns
    // userspace service lifecycle through the ServiceStart syscall.
    for id in [
        ServiceId::Display, ServiceId::Input, ServiceId::Storage, ServiceId::Network,
        ServiceId::Power, ServiceId::Modem, ServiceId::Audio, ServiceId::Camera, ServiceId::Sensor,
        ServiceId::Compositor, ServiceId::Launcher,
    ] {
        SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
            id, task: TaskId(0), state: ServiceState::Declared, generation: 0
        });
    }
    crate::serial_println!("[SERVICE] Service definitions registered; PID 1 controls startup.");
}

/// Resource budget assigned to each first-party system service. These limits are
/// process-scoped and intentionally conservative so a faulty service cannot consume
/// the same address-space, IPC, descriptor, or thread budget as an unrestricted app.
pub fn resource_limits(id: ServiceId) -> ResourceLimits {
    match id {
        ServiceId::Display | ServiceId::Compositor => ResourceLimits { max_threads: 8, max_open_files: 64, max_ipc_endpoints: 32, max_address_space_bytes: 64 * 1024 * 1024 },
        ServiceId::Storage => ResourceLimits { max_threads: 4, max_open_files: 128, max_ipc_endpoints: 32, max_address_space_bytes: 48 * 1024 * 1024 },
        ServiceId::Network => ResourceLimits { max_threads: 8, max_open_files: 64, max_ipc_endpoints: 32, max_address_space_bytes: 64 * 1024 * 1024 },
        ServiceId::Launcher => ResourceLimits { max_threads: 8, max_open_files: 64, max_ipc_endpoints: 48, max_address_space_bytes: 64 * 1024 * 1024 },
        _ => ResourceLimits { max_threads: 4, max_open_files: 32, max_ipc_endpoints: 16, max_address_space_bytes: 32 * 1024 * 1024 },
    }
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
    let desc = descriptor(id);
    if desc.abi_version != SERVICE_ABI_VERSION { return Err("unsupported service ABI"); }
    for dependency in desc.dependencies {
        let dep = SERVICE_RECORDS.lock()[*dependency as usize].ok_or("service dependency not declared")?;
        if dep.state != ServiceState::Running || dep.task == TaskId(0) { return Err("service dependency is not running"); }
    }
    let generation = SERVICE_RECORDS.lock()[id as usize].map(|r| r.generation.saturating_add(1)).unwrap_or(1);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Starting, generation });
    let task_result = crate::task::user::spawn_userspace_service(id as u8, parent_id);
    let task = match task_result {
        Ok(task) if task != TaskId(0) => task,
        _ => {
            SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord {
                id,
                task: TaskId(0),
                state: ServiceState::Failed,
                generation,
            });
            return Err("service spawn failed");
        }
    };
    crate::ipc::IPC_ROUTER.lock().register_task(task);
    let process = match crate::task::process::process_for_task(task) {
        Some(process) => process,
        None => {
            let mut scheduler = SCHEDULER.lock();
            let _ = scheduler.terminate_task(task, -1);
            let _ = scheduler.reap_child(parent_id, task);
            SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
            return Err("service process identity missing");
        }
    };
    if let Err(_) = crate::task::process::set_limits(process, resource_limits(id)) {
        let mut scheduler = SCHEDULER.lock();
        let _ = scheduler.terminate_task(task, -1);
        let _ = scheduler.reap_child(parent_id, task);
        SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
        return Err("service resource limits rejected");
    }
    let now = crate::interrupts::get_system_ticks();
    SERVICE_HEARTBEATS[id as usize].store(now, Ordering::Release);
    SERVICE_STARTED_AT[id as usize].store(now, Ordering::Release);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running, generation });
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

    let generation = existing.map(|r| r.generation.saturating_add(1)).unwrap_or(1);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Starting, generation });

    let task = match crate::task::user::spawn_verified_rapp_process(manifest, payload, parent_id) {
        Ok(task) if task != TaskId(0) => task,
        _ => {
            SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
            return Err("verified service package spawn failed");
        }
    };

    crate::ipc::IPC_ROUTER.lock().register_task(task);
    let process = crate::task::process::process_for_task(task).ok_or("service process identity missing")?;
    if crate::task::process::set_limits(process, resource_limits(id)).is_err() {
        let mut scheduler = SCHEDULER.lock();
        let _ = scheduler.terminate_task(task, -1);
        let _ = scheduler.reap_child(parent_id, task);
        SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
        return Err("service resource limits rejected");
    }
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    // Reuse the generation computed for the Starting transition; a successful
    // launch is one lifecycle transition, not a second generation bump.
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running, generation });
    Ok(task)
}

/// Stops a service task without allowing userspace to terminate arbitrary tasks.
pub fn stop(id: ServiceId, parent_id: TaskId) -> Result<(), &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    if id == ServiceId::Init { return Err("init cannot be stopped"); }
    let record = SERVICE_RECORDS.lock()[id as usize].ok_or("service not declared")?;
    if record.task == TaskId(0) { return Ok(()); }
    for dependent in 1..SERVICE_RECORDS.lock().len() {
        let Some(other) = SERVICE_RECORDS.lock()[dependent] else { continue; };
        if other.state == ServiceState::Running && descriptor(other.id).dependencies.iter().any(|dep| *dep == id) {
            return Err("service has running dependents");
        }
    }
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
    let generation = record.generation.saturating_add(1);
    SERVICE_RECORDS.lock()[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Stopped, generation });
    Ok(())
}


/// Returns stable public metadata without exposing scheduler task identity.
pub fn info(id: ServiceId) -> Option<ServiceInfo> {
    let record = SERVICE_RECORDS.lock()[id as usize]?;
    let desc = descriptor(id);
    Some(ServiceInfo { id, abi_version: desc.abi_version, state: record.state, restart: desc.restart, critical: desc.critical, generation: record.generation })
}

pub fn lookup(service_id: u8) -> Option<ServiceInfo> {
    let id = match service_id { 0 => ServiceId::Init, 1 => ServiceId::Display, 2 => ServiceId::Input, 3 => ServiceId::Storage, 4 => ServiceId::Network, 5 => ServiceId::Power, 6 => ServiceId::Modem, 7 => ServiceId::Audio, 8 => ServiceId::Camera, 9 => ServiceId::Compositor, 10 => ServiceId::Launcher, 11 => ServiceId::Sensor, _ => return None };
    info(id)
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
    let mut states = [None; 12];
    let records = *SERVICE_RECORDS.lock();
    for idx in 1..records.len() {
        let Some(record) = records[idx] else { continue; };
        if record.state == ServiceState::Starting {
            let started = SERVICE_STARTED_AT[idx].load(Ordering::Acquire);
            states[idx] = Some(if now.saturating_sub(started) > SERVICE_START_TIMEOUT_TICKS { ServiceState::Failed } else { ServiceState::Starting });
            continue;
        }
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
            if record.state == ServiceState::Running && state == ServiceState::Failed {
                SERVICE_FAILURES[idx].fetch_add(1, Ordering::Relaxed);
            }
            registry[idx] = Some(ServiceRecord { id: record.id, task: record.task, state, generation: record.generation });
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
    let mut w = W { b: out, n: 0 };
    for id in 0u8..=11u8 {
        if let Some(info) = lookup(id) {
            let state = match info.state { ServiceState::Declared => "declared", ServiceState::Starting => "starting", ServiceState::Running => "running", ServiceState::Stopped => "stopped", ServiceState::Failed => "failed" };
            let age = crate::interrupts::get_system_ticks().saturating_sub(last_heartbeat(info.id));
            let _ = writeln!(w, "{}|{}|{}|{}|{}|{}|{}|{}", info.id as u8, info.abi_version, state, info.restart as u8, info.critical as u8, info.generation, age, SERVICE_FAILURES[id as usize].load(Ordering::Relaxed));
        }
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
                SERVICE_FAILURES[idx].fetch_add(1, Ordering::Relaxed);
                records[idx] = Some(ServiceRecord { id: record.id, task: task_id, state: ServiceState::Failed, generation: record.generation });
                return;
            }
        }
    }
}

/// Recovers one failed service when explicitly requested by PID 1.
pub fn recover_failed(id: ServiceId, parent_id: TaskId) -> Result<(), &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.lock();
    if id == ServiceId::Init { return Err("init cannot be recovered"); }
    let idx = id as usize;
    let record = SERVICE_RECORDS.lock()[idx].ok_or("service not declared")?;
    if record.state != ServiceState::Failed { return Err("service is not failed"); }
    let now = crate::interrupts::get_system_ticks();
    if now < SERVICE_RETRY_AFTER[idx].load(Ordering::Acquire) { return Err("service recovery backoff active"); }
    SERVICE_RETRY_AFTER[idx].store(now.saturating_add(SERVICE_RESTART_BACKOFF_TICKS), Ordering::Release);
    if record.task != TaskId(0) {
        let scheduler = SCHEDULER.lock();
        let task = scheduler.get_task(record.task).ok_or("failed service task not found")?;
        if task.parent_id != parent_id { return Err("failed service is owned by another parent"); }
        drop(scheduler);
        let mut scheduler = SCHEDULER.lock();
        let _ = scheduler.terminate_task(record.task, -1);
        let _ = scheduler.reap_child(parent_id, record.task);
    }
    SERVICE_RECORDS.lock()[idx] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Declared, generation: record.generation });
    start_locked(id, parent_id).map(|_| ())
}
