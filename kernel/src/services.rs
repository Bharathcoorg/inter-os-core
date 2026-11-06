// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
    Inference = 12,
    Search = 13,
    Agent = 14,
    Voice = 15,
}

impl ServiceId {
    pub const fn from_u8(id: u8) -> Option<Self> {
        match id {
            0 => Some(ServiceId::Init),
            1 => Some(ServiceId::Display),
            2 => Some(ServiceId::Input),
            3 => Some(ServiceId::Storage),
            4 => Some(ServiceId::Network),
            5 => Some(ServiceId::Power),
            6 => Some(ServiceId::Modem),
            7 => Some(ServiceId::Audio),
            8 => Some(ServiceId::Camera),
            9 => Some(ServiceId::Compositor),
            10 => Some(ServiceId::Launcher),
            11 => Some(ServiceId::Sensor),
            12 => Some(ServiceId::Inference),
            13 => Some(ServiceId::Search),
            14 => Some(ServiceId::Agent),
            15 => Some(ServiceId::Voice),
            _ => None,
        }
    }
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
        ServiceId::Inference | ServiceId::Search | ServiceId::Agent | ServiceId::Voice => {
            ServiceDescriptor { id, abi_version: SERVICE_ABI_VERSION, restart: RestartPolicy::OnFailure, critical: false, dependencies: NO_DEPS }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCapabilitySpec {
    pub cap_type: CapabilityType,
    pub rights: Rights,
    pub object_id: u64,
}

// Service capability matrix isolation:
// forbidden:CapabilityType::ProcessKill
// forbidden:CapabilityType::ProcessSpawn
// NetworkSend/NetworkReceive are reserved for netd's explicit data-plane policy.
pub fn capability_policy(id: ServiceId, parent_id: TaskId) -> Result<Vec<ServiceCapabilitySpec>, &'static str> {
    let mut policy = Vec::with_capacity(20);
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
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemWrite, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FrameBufferDirect, rights: Rights::READ | Rights::WRITE, object_id: 1 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::EventPoll, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::ProcessSpawn, rights: Rights::INVOKE, object_id: 0 });
            // WHY the launcher needs the clock: the lock screen is dominated by the
            // time and the date, and without this authority it could only render the
            // layout with both absent. Read-only, and scoped to the clock, so it
            // confers no power, crypto or telephony authority.
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::RealtimeClockRead, rights: Rights::READ, object_id: 0 });
            // WHY the shell may read another app's screen: the task switcher
            // shows a live preview of what each running app is showing, and
            // the app it previews is by definition running in another task.
            // This is the only grant of this authority in the system, so an
            // app that wants to see another app's screen has to ask for it
            // and the shell has to be granted it by name.
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SurfaceCapture, rights: Rights::READ, object_id: 0 });
            // The launcher hosts the lock screen and credential setup views, requiring
            // authority to query lock state, authenticate credentials, and set PINs for the user.
            policy.push(ServiceCapabilitySpec {
                cap_type: CapabilityType::SystemControl,
                rights: Rights::INVOKE,
                object_id: crate::syscall::user_control_object(crate::user::UserId::OWNER.0),
            });
            policy.push(ServiceCapabilitySpec {
                cap_type: CapabilityType::SystemControl,
                rights: Rights::INVOKE,
                object_id: crate::syscall::user_control_object(crate::user::UserId::GUEST.0),
            });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_POWER_CONTROL_OBJECT });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_AUDIO_CONTROL_OBJECT });
            // WHY the shell owns the notification control object: the shade's
            // "Clear all" and per-card "Dismiss" post an empty package id, which is
            // a claim over every package's records and so is gated on this object.
            // Nothing was ever granted it, so both controls were refused, the
            // person tapped Clear and the notifications came back the next time the
            // shade was pulled -- and because dismissal never reached the kernel, the
            // record store could only ever grow. Scoped to the notification object,
            // so it confers no power, audio or device-wide administration.
            policy.push(ServiceCapabilitySpec {
                cap_type: CapabilityType::SystemControl,
                rights: Rights::INVOKE,
                object_id: crate::syscall::NOTIFICATION_CONTROL_OBJECT,
            });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::InferenceService, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SearchService, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
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
        // The audio service owns the feedback/mixer control object and is the
        // one service allowed to drive the AC'97 PCM ring. WHY a separate
        // `AudioStream` grant instead of widening SystemControl: emitting PCM
        // is an output side effect, not power or telephony policy, and a
        // service that only plays audio must not be able to reach those.
        ServiceId::Audio => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_AUDIO_CONTROL_OBJECT });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::AudioStream, rights: Rights::READ | Rights::WRITE, object_id: 0 });
        }
        ServiceId::Camera => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_CAMERA_CONTROL_OBJECT }),
        ServiceId::Sensor => policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SystemControl, rights: Rights::INVOKE, object_id: SERVICE_SENSOR_CONTROL_OBJECT }),
        ServiceId::Inference => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::InferenceService, rights: Rights::READ | Rights::WRITE | Rights::INVOKE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::MemoryHeap, rights: Rights::READ | Rights::WRITE | Rights::INVOKE, object_id: 0 });
            // WHY the inference service may claim a bulk channel: a model answer is
            // not 64 bytes, and 64 bytes is everything an IPC mailbox message
            // carries, so an answer sent by IPC alone arrives cut off and the
            // person reads half a sentence. The service streams the bytes over the
            // shared-memory pipe the platform already provides and posts one short
            // notice naming the channel and its length. Producer only: reading a
            // pipe is the separate consumer capability, which this service does not
            // hold, so it cannot take back what it handed out.
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::ShmProducer, rights: Rights::WRITE, object_id: 0 });
        }
        ServiceId::Search => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::ContentIndexRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::SearchService, rights: Rights::READ | Rights::WRITE | Rights::INVOKE, object_id: 0 });
        }
        ServiceId::Agent => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::FileSystemRead, rights: Rights::READ, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::AgentTask, rights: Rights::READ | Rights::WRITE | Rights::INVOKE, object_id: 0 });
        }
        ServiceId::Voice => {
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::AudioStream, rights: Rights::READ | Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::AudioCapture, rights: Rights::READ | Rights::INVOKE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 0 });
            policy.push(ServiceCapabilitySpec { cap_type: CapabilityType::VoiceService, rights: Rights::READ | Rights::WRITE | Rights::INVOKE, object_id: 0 });
        }
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

pub static SERVICE_RECORDS: crate::sync::Mutex<[Option<ServiceRecord>; 16]> = crate::sync::Mutex::new([None; 16]);

/// Locking convention for this registry (read before adding a blocking call).
///
/// WHY every acquisition below is `try_lock`: these paths are reached from
/// syscalls, the timer tick and supervision alike, and none of them may wait
/// for a holder it cannot preempt. Contended fast paths report
/// `"service registry busy"` / `"scheduler busy"` and periodic work
/// (supervision, exit accounting) skips the round; heartbeats and supervision
/// are idempotent across rounds, so a skipped beat is a delay, never a lie.
///
/// This convention is older than the syscall entry stub's `sti` (before it,
/// a blocking wait here with `RFLAGS.IF=0` wedged the whole guest). It stays
/// because a skipped round is still cheaper than a wait on this path.
///
/// `is_contention` lets syscall wrappers report these transient errors as
/// `NotReady` instead of misreporting them as permission or device failures.
pub fn is_contention(reason: &'static str) -> bool {
    matches!(
        reason,
        "service registry busy"
            | "service lifecycle busy"
            | "scheduler busy"
            | "ipc router busy"
    )
}
const SERVICE_RESTART_BACKOFF_TICKS: u64 = 1000;
static SERVICE_HEARTBEATS: [core::sync::atomic::AtomicU64; 16] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];
static SERVICE_FAILURES: [core::sync::atomic::AtomicU64; 16] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];
static SERVICES_INITIALIZED: AtomicBool = AtomicBool::new(false);
// Serializes lifecycle transitions so start/stop/recovery cannot observe and mutate
// the same service record concurrently. Scheduler locks remain scoped inside this
// boundary; callers never hold the lifecycle lock across context switches.
static SERVICE_LIFECYCLE_LOCK: crate::sync::Mutex<()> = crate::sync::Mutex::new(());
// Restart backoff prevents a failed service from consuming the scheduler in a crash loop.
static SERVICE_STARTED_AT: [core::sync::atomic::AtomicU64; 16] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];
static SERVICE_RETRY_AFTER: [core::sync::atomic::AtomicU64; 16] = [
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

/// Registers the system-service set. PID1 owns process creation and lifecycle.
pub fn init() {
    if SERVICES_INITIALIZED.swap(true, Ordering::AcqRel) { return; }
    {
        let Some(mut records) = SERVICE_RECORDS.try_lock() else { return; };
        records[ServiceId::Init as usize] = Some(ServiceRecord { id: ServiceId::Init, task: TaskId(0), state: ServiceState::Running, generation: 1 });
    }
    // Service definitions are registered but not started here. PID 1 owns
    // userspace service lifecycle through the ServiceStart syscall.
    for id in [
        ServiceId::Display, ServiceId::Input, ServiceId::Storage, ServiceId::Network,
        ServiceId::Power, ServiceId::Modem, ServiceId::Audio, ServiceId::Camera, ServiceId::Sensor,
        ServiceId::Compositor, ServiceId::Launcher, ServiceId::Inference,
    ] {
        if let Some(mut records) = SERVICE_RECORDS.try_lock() {
            records[id as usize] = Some(ServiceRecord {
                id, task: TaskId(0), state: ServiceState::Declared, generation: 0
            });
        }
    }
    crate::serial_println!("[SERVICE] Service definitions registered; PID 1 controls startup.");
}

/// Resource budget assigned to each first-party system service. These limits are
/// process-scoped and intentionally conservative so a faulty service cannot consume
/// the same address-space, IPC, descriptor, or thread budget as an unrestricted app.
pub fn resource_limits(id: ServiceId) -> ResourceLimits {
    match id {
        ServiceId::Display | ServiceId::Compositor => ResourceLimits {
            max_threads: 8,
            max_open_files: 64,
            max_ipc_endpoints: 32,
            max_address_space_bytes: 64 * 1024 * 1024,
            ..ResourceLimits::default()
        },
        ServiceId::Storage => ResourceLimits {
            max_threads: 4,
            max_open_files: 128,
            max_ipc_endpoints: 32,
            max_address_space_bytes: 48 * 1024 * 1024,
            ..ResourceLimits::default()
        },
        ServiceId::Network => ResourceLimits {
            max_threads: 8,
            max_open_files: 64,
            max_ipc_endpoints: 32,
            max_address_space_bytes: 64 * 1024 * 1024,
            ..ResourceLimits::default()
        },
        ServiceId::Launcher => ResourceLimits {
            max_threads: 8,
            max_open_files: 64,
            max_ipc_endpoints: 48,
            max_address_space_bytes: 64 * 1024 * 1024,
            ..ResourceLimits::default()
        },
        ServiceId::Inference => ResourceLimits {
            max_threads: 4,
            max_open_files: 32,
            max_ipc_endpoints: 32,
            max_address_space_bytes: crate::task::user::user_address_space_limit() as u64,
            ..ResourceLimits::default()
        },
        _ => ResourceLimits {
            max_threads: 4,
            max_open_files: 32,
            max_ipc_endpoints: 16,
            max_address_space_bytes: 32 * 1024 * 1024,
            ..ResourceLimits::default()
        },
    }
}

/// Starts one declared service on behalf of userspace PID 1.
pub fn start(id: ServiceId, parent_id: TaskId) -> Result<TaskId, &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.try_lock().ok_or("service lifecycle busy")?;
    start_locked(id, parent_id)
}

fn start_locked(id: ServiceId, parent_id: TaskId) -> Result<TaskId, &'static str> {
    if id == ServiceId::Init { return Ok(TaskId(0)); }
    // A service start that fails must say why on the service console. A silent
    // Err here leaves PID 1 unable to start anything and the device with no
    // display, input or launcher, with nothing to explain it.
    macro_rules! bail {
        ($reason:expr) => {{
            let reason: &'static str = $reason;
            crate::serial_println!("[SERVICE] start({:?}) rejected: {}", id, reason);
            return Err(reason);
        }};
    }
    let existing = SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize];
    if let Some(record) = existing {
        if record.state == ServiceState::Running && record.task != TaskId(0) {
            // A lifecycle manager may only observe/reuse services owned by the
            // same PID1 parent. Never turn the idempotent start path into a
            // cross-parent task-disclosure or control primitive.
            let scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
            let task = match scheduler.get_task(record.task) {
                Some(task) => task,
                None => bail!("running service task not found"),
            };
            if task.parent_id != parent_id {
                bail!("running service is not owned by caller");
            }
            return Ok(record.task);
        }
        if record.state == ServiceState::Starting { bail!("service already starting"); }
        // A failed userspace service still owns a scheduler task until the explicit
        // recovery path tears it down and reaps it. Do not spawn another
        // process here or recovery could create duplicate service instances.
        if record.state == ServiceState::Failed && record.task != TaskId(0) {
            bail!("failed service requires recovery");
        }
        if !matches!(record.state, ServiceState::Declared | ServiceState::Stopped | ServiceState::Failed) {
            bail!("service is not startable");
        }
    } else { bail!("service not declared"); }
    let desc = descriptor(id);
    if desc.abi_version != SERVICE_ABI_VERSION { bail!("unsupported service ABI"); }
    for dependency in desc.dependencies {
        let dep = match SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[*dependency as usize] {
            Some(dep) => dep,
            None => bail!("service dependency not declared"),
        };
        if dep.state != ServiceState::Running || dep.task == TaskId(0) {
            bail!("service dependency is not running");
        }
    }
    let generation = SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize].map(|r| r.generation.saturating_add(1)).unwrap_or(1);
    SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Starting, generation });
    let task = match crate::task::user::spawn_userspace_service(id as u8, parent_id) {
        Ok(task) if task != TaskId(0) => task,
        Ok(_) => bail!("service spawn returned no task"),
        Err(reason) => bail!(reason),
    };
    crate::ipc::IPC_ROUTER.try_lock().ok_or("ipc router busy")?.register_task(task);
    let process = match crate::task::process::process_for_task(task) {
        Some(process) => process,
        None => {
            let mut scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
            let _ = scheduler.terminate_task(task, -1);
            let _ = scheduler.reap_child(parent_id, task);
            SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
            bail!("service process identity missing");
        }
    };
    if crate::task::process::set_limits(process, resource_limits(id)).is_err() {
        let mut scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
        let _ = scheduler.terminate_task(task, -1);
        let _ = scheduler.reap_child(parent_id, task);
        SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
        bail!("service resource limits rejected");
    }
    let now = crate::interrupts::get_system_ticks();
    SERVICE_HEARTBEATS[id as usize].store(now, Ordering::Release);
    SERVICE_STARTED_AT[id as usize].store(now, Ordering::Release);
    SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running, generation });
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
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.try_lock().ok_or("service lifecycle busy")?;

    if id == ServiceId::Init {
        return Err("init is not a package-launched service");
    }

    let existing = SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize];
    if let Some(record) = existing {
        if record.state == ServiceState::Running && record.task != TaskId(0) {
            let scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
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
    SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Starting, generation });

    let task = match crate::task::user::spawn_verified_rapp_process(manifest, payload, parent_id) {
        Ok(task) if task != TaskId(0) => task,
        _ => {
            SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
            return Err("verified service package spawn failed");
        }
    };

    crate::ipc::IPC_ROUTER.try_lock().ok_or("ipc router busy")?.register_task(task);
    let process = match crate::task::process::process_for_task(task) {
        Some(process) => process,
        None => {
            let mut scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
            let _ = scheduler.terminate_task(task, -1);
            let _ = scheduler.reap_child(parent_id, task);
            SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
            return Err("service process identity missing");
        }
    };
    if crate::task::process::set_limits(process, resource_limits(id)).is_err() {
        let mut scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
        let _ = scheduler.terminate_task(task, -1);
        let _ = scheduler.reap_child(parent_id, task);
        SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Failed, generation });
        return Err("service resource limits rejected");
    }
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    // Reuse the generation computed for the Starting transition; a successful
    // launch is one lifecycle transition, not a second generation bump.
    SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running, generation });
    Ok(task)
}

/// Stops a service task without allowing userspace to terminate arbitrary tasks.
pub fn stop(id: ServiceId, parent_id: TaskId) -> Result<(), &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.try_lock().ok_or("service lifecycle busy")?;
    if id == ServiceId::Init { return Err("init cannot be stopped"); }
    let record = SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize].ok_or("service not declared")?;
    if record.task == TaskId(0) { return Ok(()); }
    let records = *SERVICE_RECORDS.try_lock().ok_or("service registry busy")?;
    for dependent in 1..records.len() {
        let Some(other) = records[dependent] else { continue; };
        if other.state == ServiceState::Running && descriptor(other.id).dependencies.iter().any(|dep| *dep == id) {
            return Err("service has running dependents");
        }
    }
    // Services are children of the PID1 task that started them. Stopping a
    // service must remain parent-scoped even when the caller has the manager
    // capability; never allow the manager to terminate another owner's task.
    let service_task = record.task;
    {
        let scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
        let task = scheduler.get_task(service_task).ok_or("service task not found")?;
        if task.parent_id != parent_id {
            return Err("service is not owned by caller");
        }
    }
    let termination_result = {
        let mut scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
        let result = scheduler.terminate_task(service_task, 0);
        if result.is_ok() { let _ = scheduler.reap_child(parent_id, record.task); }
        result
    };
    termination_result.map_err(|_| "service termination failed")?;
    let generation = record.generation.saturating_add(1);
    SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Stopped, generation });
    Ok(())
}


/// Returns stable public metadata without exposing scheduler task identity.
pub fn info(id: ServiceId) -> Option<ServiceInfo> {
    let record = SERVICE_RECORDS.try_lock()?[id as usize]?;
    let desc = descriptor(id);
    Some(ServiceInfo { id, abi_version: desc.abi_version, state: record.state, restart: desc.restart, critical: desc.critical, generation: record.generation })
}

pub fn lookup(service_id: u8) -> Option<ServiceInfo> {
    let id = match service_id {
        0 => ServiceId::Init,
        1 => ServiceId::Display,
        2 => ServiceId::Input,
        3 => ServiceId::Storage,
        4 => ServiceId::Network,
        5 => ServiceId::Power,
        6 => ServiceId::Modem,
        7 => ServiceId::Audio,
        8 => ServiceId::Camera,
        9 => ServiceId::Compositor,
        10 => ServiceId::Launcher,
        11 => ServiceId::Sensor,
        12 => ServiceId::Inference,
        13 => ServiceId::Search,
        14 => ServiceId::Agent,
        15 => ServiceId::Voice,
        _ => return None,
    };
    info(id)
}

/// Returns a task ID for a named service when registered.
pub fn task_id(id: ServiceId) -> Option<TaskId> {
    SERVICE_RECORDS.try_lock()?[id as usize].map(|r| r.task)
}

/// Records a service heartbeat. Service workers call this before yielding so
/// the supervisor can distinguish a live worker from a dead/stopped task.
#[allow(dead_code)]
pub(crate) fn heartbeat(id: ServiceId) {
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
}

/// Reports whether the task a record names is still a live worker.
fn worker_is_live(task_id: TaskId) -> bool {
    let Some(scheduler) = SCHEDULER.try_lock() else { return false; };
    match scheduler.get_task(task_id) {
        Some(task) => !matches!(task.state, crate::task::TaskState::Terminated | crate::task::TaskState::Zombie),
        None => false,
    }
}

pub fn heartbeat_for_task(id: ServiceId, task_id: TaskId) -> Result<(), &'static str> {
    let record = SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize].ok_or("service not declared")?;
    if record.task != task_id { return Err("service task mismatch"); }
    // WHY a Failed record answers to its own worker's heartbeat: supervision turns
    // Running into Failed the moment a heartbeat ages out, and one busy stretch at
    // boot is enough to age every service out at least once. Refusing the heartbeat
    // from there made that reading permanent -- the worker kept serving, PID 1 could
    // not recover it either (recovery demands a terminal task, start demands no task
    // at all), and anything that gates on the registry stops asking the service at
    // all. A heartbeat from the very task the record names is the direct evidence
    // supervision is missing, so it revives that record. A task that actually died
    // cannot send one, so the real failure path and PID 1's recovery of it are
    // untouched.
    if record.state == ServiceState::Failed && !worker_is_live(task_id) {
        return Err("service task is not live");
    }
    SERVICE_HEARTBEATS[id as usize].store(crate::interrupts::get_system_ticks(), Ordering::Release);
    if record.state == ServiceState::Failed {
        SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[id as usize] =
            Some(ServiceRecord { id, task: task_id, state: ServiceState::Running, generation: record.generation });
    }
    Ok(())
}


/// Supervises all registered services and marks missing, terminated, or
/// heartbeat-stale workers as failed. Recovery remains an explicit PID 1 action.
/// Best-effort per round: every lock here is `try_lock`, so a contended round
/// is skipped instead of wedging the caller (see the registry convention).
pub fn supervise() {
    let _lifecycle = match SERVICE_LIFECYCLE_LOCK.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    let now = crate::interrupts::get_system_ticks();
    let mut sched = match SCHEDULER.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    sched.reap_orphans();
    let mut states = [None; 16];
    let records = match SERVICE_RECORDS.try_lock() {
        Some(guard) => *guard,
        None => return,
    };
    for idx in 1..records.len() {
        let Some(record) = records[idx] else { continue; };
        if record.state == ServiceState::Starting {
            let started = SERVICE_STARTED_AT[idx].load(Ordering::Acquire);
            states[idx] = Some(if now.saturating_sub(started) > SERVICE_START_TIMEOUT_TICKS {
                ServiceState::Failed
            } else {
                ServiceState::Starting
            });
            continue;
        }
        if !matches!(record.state, ServiceState::Running) {
            states[idx] = Some(record.state);
            continue;
        }
        let stale = now.saturating_sub(SERVICE_HEARTBEATS[idx].load(Ordering::Acquire)) > HEARTBEAT_TIMEOUT_TICKS;
        let failed = match sched.get_task(record.task) {
            Some(task) => matches!(task.state, crate::task::TaskState::Terminated | crate::task::TaskState::Zombie),
            None => true,
        };
        states[idx] = Some(if failed || stale { ServiceState::Failed } else { ServiceState::Running });
    }
    drop(sched);
    let mut registry = match SERVICE_RECORDS.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    for idx in 1..registry.len() {
        if let (Some(record), Some(state)) = (records[idx], states[idx]) {
            if record.state == ServiceState::Running && state == ServiceState::Failed {
                SERVICE_FAILURES[idx].fetch_add(1, Ordering::Relaxed);
                CONFIRMATION_ATTEMPTED.store(false, Ordering::Release);
            }
            registry[idx] = Some(ServiceRecord { id: record.id, task: record.task, state, generation: record.generation });
        }
    }
    let all_critical_running = (1..registry.len()).all(|idx| match registry[idx] {
        Some(record) => !descriptor(record.id).critical || record.state == ServiceState::Running,
        None => false,
    });
    drop(registry);
    static CONFIRMATION_ATTEMPTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if all_critical_running && !CONFIRMATION_ATTEMPTED.load(Ordering::Acquire) {
        let mut recovery = match crate::recovery::BOOT_RECOVERY.try_lock() {
            Some(guard) => guard,
            None => return,
        };
        if recovery.record.outcome != crate::recovery::BootOutcome::Success {
            CONFIRMATION_ATTEMPTED.store(true, Ordering::Release);
            match recovery.confirm_boot() {
                Ok(()) => crate::serial_println!("[RECOVERY] boot health confirmed after all critical services reached Running."),
                Err(error) => crate::serial_println!("[RECOVERY] boot health confirmation deferred because durable state is unavailable ({:?}).", error),
            }
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
    for id in 0u8..=15u8 {
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
/// Called from the scheduler exit path, so it must never block: a contended
/// round is skipped and the exit surfaces via heartbeat-stale supervision.
pub fn record_exit(task_id: TaskId, _exit_code: i32) {
    let _lifecycle = match SERVICE_LIFECYCLE_LOCK.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    let mut records = match SERVICE_RECORDS.try_lock() {
        Some(guard) => guard,
        None => return,
    };
    for idx in 1..records.len() {
        if let Some(record) = records[idx] {
            if record.task == task_id {
                SERVICE_FAILURES[idx].fetch_add(1, Ordering::Relaxed);
                // Keep the failed task identity until PID 1 explicitly reaps it.
                // Recovery uses this identity to prove ownership and avoid spawning
                // a replacement while the old process is still live.
                records[idx] = Some(ServiceRecord { id: record.id, task: task_id, state: ServiceState::Failed, generation: record.generation });
                return;
            }
        }
    }
}

/// Recovers one failed service when explicitly requested by PID 1.
pub fn recover_failed(id: ServiceId, parent_id: TaskId) -> Result<(), &'static str> {
    let _lifecycle = SERVICE_LIFECYCLE_LOCK.try_lock().ok_or("service lifecycle busy")?;
    if id == ServiceId::Init { return Err("init cannot be recovered"); }
    let idx = id as usize;
    let record = SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[idx].ok_or("service not declared")?;
    if record.state != ServiceState::Failed { return Err("service is not failed"); }
    let now = crate::interrupts::get_system_ticks();
    if now < SERVICE_RETRY_AFTER[idx].load(Ordering::Acquire) { return Err("service recovery backoff active"); }
    if record.task != TaskId(0) {
        let scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
        let task = scheduler.get_task(record.task).ok_or("failed service task not found")?;
        if task.parent_id != parent_id {
            return Err("failed service is owned by another parent");
        }
        if !matches!(task.state, crate::task::TaskState::Zombie | crate::task::TaskState::Terminated) {
            return Err("failed service task is not terminal");
        }
        drop(scheduler);
        let mut scheduler = SCHEDULER.try_lock().ok_or("scheduler busy")?;
        let _ = scheduler.reap_child(parent_id, record.task);
    }
    // Consume one bounded recovery attempt only after ownership and terminal-state
    // checks succeed. A rejected cross-parent recovery must not penalize PID1's
    // retry budget.
    SERVICE_RETRY_AFTER[idx].store(now.saturating_add(SERVICE_RESTART_BACKOFF_TICKS), Ordering::Release);
    SERVICE_RECORDS.try_lock().ok_or("service registry busy")?[idx] = Some(ServiceRecord { id, task: TaskId(0), state: ServiceState::Declared, generation: record.generation });
    start_locked(id, parent_id).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskId;

    /// The launcher must be able to read the clock, and only the clock.
    ///
    /// WHY this exists: `sys_rtc_get_time` used to require `SystemControl` through
    /// the object-blind `require_system_control()`. The only way to let the lock
    /// screen render a time was therefore to grant the launcher power, crypto and
    /// telephony authority over the entire device. Reading the clock authorises
    /// nothing, so it gets its own narrow read-only capability.
    ///
    /// The second assertion is the one that matters most: granting the clock must
    /// NOT have handed the launcher general system control, because that is the
    /// over-grant this change exists to avoid.
    #[test]
    fn launcher_can_read_the_clock_without_holding_system_control() {
        let policy = capability_policy(ServiceId::Launcher, TaskId(1)).expect("launcher policy");

        assert!(
            policy
                .iter()
                .any(|c| c.cap_type == CapabilityType::RealtimeClockRead && c.rights.contains(Rights::READ)),
            "the launcher needs read authority for the real-time clock or the lock screen cannot show a time"
        );

        assert!(
            policy.iter().any(|c| c.cap_type == CapabilityType::SystemControl && c.object_id == SERVICE_POWER_CONTROL_OBJECT && c.rights.contains(Rights::INVOKE)),
            "the launcher must hold the narrowly-scoped power control capability for real restart/shutdown/display actions"
        );
        assert!(
            policy.iter().any(|c| c.cap_type == CapabilityType::SystemControl && c.object_id == SERVICE_AUDIO_CONTROL_OBJECT && c.rights.contains(Rights::INVOKE)),
            "the launcher needs narrowly-scoped audio authority for real volume controls"
        );
        assert!(
            !policy.iter().any(|c| c.cap_type == CapabilityType::SystemControl && c.object_id == SERVICE_TELEPHONY_CONTROL_OBJECT),
            "the launcher must not hold telephony service authority"
        );

        assert!(
            !policy
                .iter()
                .any(|c| c.cap_type == CapabilityType::RealtimeClockRead && c.rights.contains(Rights::WRITE)),
            "the clock capability must be read-only"
        );
    }

    /// The shell must be able to clear the shade it draws.
    ///
    /// WHY this is its own test: the shade's Clear all and per-card Dismiss post an
    /// empty package id, which is a claim over every package's records, so the
    /// syscall gates it on the notification control object. Nothing held that object,
    /// so both controls were refused and a person could not dismiss a notification at
    /// all -- which is also how the record store became append-only in practice. The
    /// grant is to that one object, and the second assertion is what keeps it that way.
    #[test]
    fn launcher_holds_the_notification_control_object_and_nothing_else_does() {
        let launcher = capability_policy(ServiceId::Launcher, TaskId(1)).expect("launcher policy");
        assert!(
            launcher
                .iter()
                .any(|c| c.cap_type == CapabilityType::SystemControl
                    && c.object_id == crate::syscall::NOTIFICATION_CONTROL_OBJECT
                    && c.rights.contains(Rights::INVOKE)),
            "the shell cannot clear the notification shade without the notification control object"
        );
        assert!(
            !launcher
                .iter()
                .any(|c| c.cap_type == CapabilityType::SystemControl
                    && c.object_id == crate::syscall::GLOBAL_SYSTEM_CONTROL_OBJECT),
            "the notification grant must not arrive with device-wide system control attached"
        );

        for id in [
            ServiceId::Display,
            ServiceId::Input,
            ServiceId::Storage,
            ServiceId::Network,
            ServiceId::Power,
            ServiceId::Modem,
            ServiceId::Audio,
            ServiceId::Camera,
            ServiceId::Compositor,
            ServiceId::Sensor,
            ServiceId::Inference,
            ServiceId::Search,
            ServiceId::Agent,
            ServiceId::Voice,
        ] {
            let Ok(policy) = capability_policy(id, TaskId(1)) else {
                continue;
            };
            assert!(
                !policy.iter().any(|c| c.object_id == crate::syscall::NOTIFICATION_CONTROL_OBJECT),
                "{:?} must not hold the notification control object",
                id
            );
        }
    }

    /// The inference service must be able to hand a whole answer to the app that
    /// asked for it.
    ///
    /// WHY this is its own test: the grant exists only because the mailbox is small.
    /// Without producer authority the service can still answer, but everything past
    /// the first 64 bytes of the reply is dropped by the kernel while the response
    /// header goes on reporting the full length -- which is a device that tells the
    /// app it sent an answer it did not send. The second assertion keeps the grant to
    /// one service, because a channel another service could claim is a channel whose
    /// bytes could be pointed at the wrong task.
    #[test]
    fn the_inference_service_can_stream_a_whole_answer() {
        let inference = capability_policy(ServiceId::Inference, TaskId(1)).expect("inference policy");
        assert!(
            inference
                .iter()
                .any(|c| c.cap_type == CapabilityType::ShmProducer && c.rights.contains(Rights::WRITE)),
            "the inference service cannot deliver an answer longer than a mailbox message"
        );
        assert!(
            !inference
                .iter()
                .any(|c| c.cap_type == CapabilityType::ShmConsumer),
            "answering a query does not need the authority to consume a pipe"
        );

        for id in [
            ServiceId::Display,
            ServiceId::Input,
            ServiceId::Storage,
            ServiceId::Network,
            ServiceId::Power,
            ServiceId::Modem,
            ServiceId::Audio,
            ServiceId::Camera,
            ServiceId::Compositor,
            ServiceId::Launcher,
            ServiceId::Sensor,
            ServiceId::Search,
            ServiceId::Agent,
            ServiceId::Voice,
        ] {
            let Ok(policy) = capability_policy(id, TaskId(1)) else {
                continue;
            };
            assert!(
                !policy.iter().any(|c| c.cap_type == CapabilityType::ShmProducer),
                "{:?} must not hold producer authority over a shared-memory channel",
                id
            );
        }
    }

    /// No other service may acquire clock authority by accident.
    ///
    /// WHY: the clock is a distinct capability precisely so it can be granted
    /// narrowly. If every service picked it up through a shared tail, the split
    /// would be meaningless and the next service added would silently inherit it.
    #[test]
    fn clock_authority_is_not_granted_to_other_services() {
        for id in [
            ServiceId::Display,
            ServiceId::Input,
            ServiceId::Storage,
            ServiceId::Network,
            ServiceId::Power,
            ServiceId::Modem,
            ServiceId::Audio,
            ServiceId::Camera,
            ServiceId::Compositor,
            ServiceId::Sensor,
            ServiceId::Inference,
            ServiceId::Search,
            ServiceId::Agent,
            ServiceId::Voice,
        ] {
            let Ok(policy) = capability_policy(id, TaskId(1)) else {
                continue;
            };
            assert!(
                !policy.iter().any(|c| c.cap_type == CapabilityType::RealtimeClockRead),
                "{:?} must not hold RealtimeClockRead",
                id
            );
        }
    }
}
