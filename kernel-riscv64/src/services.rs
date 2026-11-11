// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V 64 Authoritative Service Registry & Lifecycle Manager
//!
//! Provides the service discovery and supervision state for PID 1 (userspace init).
//! Tracks declared system services and coordinates lifecycle state transitions.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

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
#[repr(u8)]
pub enum ServiceState {
    Stopped = 0,
    Starting = 1,
    Running = 2,
    Degraded = 3,
    Crashed = 4,
    Restarting = 5,
}

impl ServiceState {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            0 => ServiceState::Stopped,
            1 => ServiceState::Starting,
            2 => ServiceState::Running,
            3 => ServiceState::Degraded,
            4 => ServiceState::Crashed,
            5 => ServiceState::Restarting,
            _ => ServiceState::Crashed,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            ServiceState::Stopped => "stopped",
            ServiceState::Starting => "starting",
            ServiceState::Running => "running",
            ServiceState::Degraded => "degraded",
            ServiceState::Crashed => "crashed",
            ServiceState::Restarting => "restarting",
        }
    }
}

pub const HEARTBEAT_TIMEOUT_TICKS: u64 = 300_000_000;
pub const MAX_RESTART_ATTEMPTS: u32 = 5;

pub struct ServiceEntry {
    pub id: ServiceId,
    pub name: &'static str,
    pub state: AtomicU8,
    pub restart: u8,
    pub critical: u8,
    pub generation: AtomicU32,
    pub last_heartbeat: AtomicU64,
    pub failures: AtomicU32,
    pub pid: AtomicU32,
    pub restart_count: AtomicU32,
    pub backoff_ticks: AtomicU64,
}

// 12 core declared services matching userspace/init discovery expectations
pub static SERVICES: [ServiceEntry; 12] = [
    ServiceEntry { id: ServiceId::Display, name: "displayd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Input, name: "inputd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Storage, name: "storaged", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Network, name: "netd", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Power, name: "powerd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Modem, name: "modemd", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Audio, name: "audiod", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Camera, name: "camerad", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Compositor, name: "compositor", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Launcher, name: "launcher", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Sensor, name: "sensord", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Inference, name: "aid", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
];

pub fn start(id: u8) -> Result<u32, ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            let old_pid = svc.pid.load(Ordering::Acquire);
            if old_pid != 0 {
                let mut sched = crate::task::SCHEDULER.lock();
                let _ = sched.reap_task(1, true, old_pid);
                drop(sched);
                svc.pid.store(0, Ordering::Release);
            }

            svc.state.store(ServiceState::Starting as u8, Ordering::Release);

            let img = match crate::images::service_image(id) {
                Some(i) => i,
                None => {
                    svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                    svc.failures.fetch_add(1, Ordering::Relaxed);
                    return Err(());
                }
            };

            let loaded = match crate::elf::load_elf(img.elf) {
                Ok(l) => l,
                Err(e) => {
                    crate::println!("[SERVICE ERROR] Failed to load '{}': {}", img.name, e);
                    svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                    svc.failures.fetch_add(1, Ordering::Relaxed);
                    return Err(());
                }
            };

            let mut sched = crate::task::SCHEDULER.lock();
            let mut caps = crate::capability::CapabilityTable::new();
            let _ = caps.mint(crate::capability::CapabilityType::IpcSend, crate::capability::Rights::INVOKE, None);
            let _ = caps.mint(crate::capability::CapabilityType::IpcReceive, crate::capability::Rights::READ, None);
            let _ = caps.mint(crate::capability::CapabilityType::SerialOutput, crate::capability::Rights::WRITE, None);
            if id == 9 || id == 10 { // compositor or launcher
                let _ = caps.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights(crate::capability::Rights::WRITE.0 | crate::capability::Rights::ADMIN.0), None);
                let _ = caps.mint(crate::capability::CapabilityType::SystemResource, crate::capability::Rights(crate::capability::Rights::READ.0 | crate::capability::Rights::WRITE.0 | crate::capability::Rights::ADMIN.0), None);
            } else {
                let _ = caps.mint(crate::capability::CapabilityType::DisplayServer, crate::capability::Rights::WRITE, None);
            }
            let _ = caps.mint(crate::capability::CapabilityType::EventPoll, crate::capability::Rights::READ, None);
            let _ = caps.mint(crate::capability::CapabilityType::InputDevice, crate::capability::Rights::READ, None);
            if id == 10 { // launcher
                let _ = caps.mint(crate::capability::CapabilityType::TimerTickRead, crate::capability::Rights::READ, None);
            }
            let tid = sched.spawn_user_process(1, img.name, loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps);
            if tid == 0 {
                drop(sched);
                svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                svc.failures.fetch_add(1, Ordering::Relaxed);
                return Err(());
            }

            let verified = sched.tasks.iter().any(|t| t.id == tid && (t.state == crate::task::TaskState::Ready || t.state == crate::task::TaskState::Running));
            drop(sched);
            if !verified {
                svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                svc.failures.fetch_add(1, Ordering::Relaxed);
                return Err(());
            }

            let ticks: u64;
            unsafe {
                core::arch::asm!("rdtime {0}", out(reg) ticks);
            }
            svc.pid.store(tid, Ordering::Release);
            svc.last_heartbeat.store(ticks, Ordering::Release);
            svc.state.store(ServiceState::Running as u8, Ordering::Release);
            svc.generation.fetch_add(1, Ordering::Relaxed);

            crate::println!("[SERVICE] RISC-V userspace service '{}' spawned (PID {})", img.name, tid);
            return Ok(tid);
        }
    }
    Err(())
}

pub fn stop(id: u8) -> Result<(), ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            let pid = svc.pid.swap(0, Ordering::AcqRel);
            if pid != 0 {
                let mut sched = crate::task::SCHEDULER.lock();
                for task in &mut sched.tasks {
                    if task.id == pid {
                        task.state = crate::task::TaskState::Terminated;
                    }
                }
                let _ = sched.reap_task(1, true, pid);
            }
            svc.state.store(ServiceState::Stopped as u8, Ordering::Release);
            return Ok(());
        }
    }
    Err(())
}

pub fn heartbeat(caller_pid: u32) -> Result<(), ()> {
    if caller_pid == 0 || caller_pid == crate::task::INVALID_PID {
        return Err(());
    }
    let ticks: u64;
    unsafe {
        core::arch::asm!("rdtime {0}", out(reg) ticks);
    }
    for svc in &SERVICES {
        if svc.pid.load(Ordering::Acquire) == caller_pid {
            svc.last_heartbeat.store(ticks, Ordering::Release);
            let prev = svc.state.load(Ordering::Acquire);
            if prev == ServiceState::Degraded as u8 {
                svc.state.store(ServiceState::Running as u8, Ordering::Release);
            }
            return Ok(());
        }
    }
    Err(())
}

pub fn recover_failed(id: u8) -> Result<(), ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            let st = svc.state.load(Ordering::Acquire);
            if st != ServiceState::Crashed as u8 && st != ServiceState::Degraded as u8 {
                return Err(());
            }
            let fails = svc.failures.load(Ordering::Relaxed);
            if fails >= MAX_RESTART_ATTEMPTS {
                crate::println!("[SERVICE] Service '{}' exceeded maximum retry budget ({})", svc.name, fails);
                return Err(());
            }
            svc.restart_count.fetch_add(1, Ordering::Relaxed);
            svc.state.store(ServiceState::Restarting as u8, Ordering::Release);
            return match start(id) {
                Ok(_) => Ok(()),
                Err(()) => {
                    svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                    Err(())
                }
            };
        }
    }
    Err(())
}

/// Takes a snapshot of all services in pipe-separated format:
/// `{id}|1|{state_str}|{restart}|{critical}|{generation}|{age}|{failures}\n`
pub fn snapshot(out: &mut [u8]) -> usize {
    use core::fmt::Write;

    let ticks: u64;
    unsafe {
        core::arch::asm!("rdtime {0}", out(reg) ticks);
    }

    struct SliceWriter<'a> {
        buf: &'a mut [u8],
        pos: usize,
    }

    impl<'a> Write for SliceWriter<'a> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let avail = self.buf.len().saturating_sub(self.pos);
            let to_copy = bytes.len().min(avail);
            self.buf[self.pos..self.pos + to_copy].copy_from_slice(&bytes[..to_copy]);
            self.pos += to_copy;
            Ok(())
        }
    }

    let mut writer = SliceWriter { buf: out, pos: 0 };
    let sched = crate::task::SCHEDULER.lock();

    for svc in &SERVICES {
        let mut state_val = svc.state.load(Ordering::Acquire);
        let pid = svc.pid.load(Ordering::Acquire);

        // Audit active process status
        if state_val == ServiceState::Running as u8 {
            let task_active = pid != 0 && sched.tasks.iter().any(|t| t.id == pid && (t.state == crate::task::TaskState::Ready || t.state == crate::task::TaskState::Running));
            if !task_active {
                svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                state_val = ServiceState::Crashed as u8;
            } else {
                let hb = svc.last_heartbeat.load(Ordering::Acquire);
                if hb > 0 && ticks.saturating_sub(hb) > HEARTBEAT_TIMEOUT_TICKS {
                    svc.state.store(ServiceState::Degraded as u8, Ordering::Release);
                    state_val = ServiceState::Degraded as u8;
                }
            }
        }

        let state_str = ServiceState::from_u8(state_val).as_str();
        let gen = svc.generation.load(Ordering::Relaxed);
        let fails = svc.failures.load(Ordering::Relaxed);
        let hb = svc.last_heartbeat.load(Ordering::Acquire);
        let age = if hb > 0 { ticks.saturating_sub(hb) / 1000 } else { 0 };
        let _ = writeln!(
            writer,
            "{}|1|{}|{}|{}|{}|{}|{}",
            svc.id as u8, state_str, svc.restart, svc.critical, gen, age, fails
        );
    }
    writer.pos
}
