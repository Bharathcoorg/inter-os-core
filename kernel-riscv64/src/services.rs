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

struct ServiceEntry {
    id: ServiceId,
    name: &'static str,
    state: AtomicU8,
    restart: u8,
    critical: u8,
    generation: AtomicU32,
    last_heartbeat: AtomicU64,
    failures: AtomicU32,
    pid: AtomicU32,
    restart_count: AtomicU32,
    backoff_ticks: AtomicU64,
}

// 12 core declared services matching userspace/init discovery expectations
static SERVICES: [ServiceEntry; 12] = [
    ServiceEntry { id: ServiceId::Display, name: "displayd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Input, name: "inputd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Storage, name: "storaged", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Network, name: "netd", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Power, name: "powerd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Modem, name: "modemd", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Audio, name: "audiod", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Camera, name: "camerad", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Compositor, name: "compositor", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Launcher, name: "launcher", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Sensor, name: "sensord", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
    ServiceEntry { id: ServiceId::Inference, name: "aid", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(0), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0), pid: AtomicU32::new(0), restart_count: AtomicU32::new(0), backoff_ticks: AtomicU64::new(0) },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceLifecycleToken {
    service_id: u8,
    pid: u32,
    generation: u32,
}

impl ServiceLifecycleToken {
    /// Internal issuance constructor, only callable from within `services.rs`
    /// while holding `SERVICE_LIFECYCLE_LOCK`.
    fn issue(entry: &ServiceEntry) -> Option<Self> {
        let pid = entry.pid.load(Ordering::Acquire);
        let generation = entry.generation.load(Ordering::Acquire);
        if pid == 0 || crate::task::is_reserved_pid(pid) {
            return None;
        }
        Some(Self {
            service_id: entry.id as u8,
            pid,
            generation,
        })
    }

    #[cfg(test)]
    pub(crate) fn forge_for_test(service_id: u8, pid: u32, generation: u32) -> Self {
        Self {
            service_id,
            pid,
            generation,
        }
    }

    pub(crate) fn service_id(&self) -> u8 {
        self.service_id
    }

    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    pub(crate) fn generation(&self) -> u32 {
        self.generation
    }

    /// Verifies that this token matches the live registered service entry under `SERVICES`.
    pub(crate) fn verify_live(&self) -> bool {
        if crate::task::is_reserved_pid(self.pid) {
            return false;
        }
        for svc in &SERVICES {
            if svc.id as u8 == self.service_id {
                let live_pid = svc.pid.load(Ordering::Acquire);
                let live_gen = svc.generation.load(Ordering::Acquire);
                return live_pid == self.pid && live_gen == self.generation;
            }
        }
        false
    }
}

/// Validate whether an ID corresponds to a registered service without leaking the array
pub fn is_valid_service_id(id: u8) -> bool {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            return true;
        }
    }
    false
}

pub fn service_pid(id: u8) -> Option<u32> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            let p = svc.pid.load(Ordering::Acquire);
            return if p != 0 { Some(p) } else { None };
        }
    }
    None
}

pub fn service_generation(id: u8) -> Option<u32> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            return Some(svc.generation.load(Ordering::Acquire));
        }
    }
    None
}

pub fn service_state(id: u8) -> Option<ServiceState> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            return Some(ServiceState::from_u8(svc.state.load(Ordering::Acquire)));
        }
    }
    None
}

pub static SERVICE_LIFECYCLE_LOCK: spin::Mutex<()> = spin::Mutex::new(());

pub fn start(id: u8) -> Result<u32, ()> {
    let _lock = SERVICE_LIFECYCLE_LOCK.lock();
    start_internal(id)
}

fn start_internal(id: u8) -> Result<u32, ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            // Atomic state transition: only Stopped, Crashed, or Restarting can transition to Starting
            let cur = svc.state.load(Ordering::Acquire);
            if cur == ServiceState::Starting as u8 || cur == ServiceState::Running as u8 {
                crate::println!("[SERVICE] Rejecting duplicate start for active service '{}'", svc.name);
                return Err(());
            }

            let transitioned = svc.state.compare_exchange(
                cur,
                ServiceState::Starting as u8,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            if transitioned.is_err() {
                crate::println!("[SERVICE] Concurrent start race lost for service '{}'", svc.name);
                return Err(());
            }

            let old_pid = svc.pid.load(Ordering::Acquire);
            if old_pid != 0 {
                let token = match ServiceLifecycleToken::issue(svc) {
                    Some(t) => t,
                    None => {
                        crate::println!("[SERVICE] Failed to issue token for old PID {} for '{}'", old_pid, svc.name);
                        svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                        return Err(());
                    }
                };
                {
                    let mut sched = crate::task::SCHEDULER.lock();
                    if let Err(e) = crate::task::terminate_service_task(&mut sched, &token) {
                        crate::println!("[SERVICE] Failed to terminate old PID {} for '{}': {}", old_pid, svc.name, e);
                        svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                        return Err(());
                    }
                }
                match crate::task::reap_service_task(&token) {
                    Ok(_) => {
                        svc.pid.store(0, Ordering::Release);
                    }
                    Err(e) => {
                        crate::println!("[SERVICE] Failed to reap old process PID {} for '{}': {}", old_pid, svc.name, e);
                        svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                        return Err(());
                    }
                }
            }

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
            let next_gen = svc.generation.load(Ordering::Acquire).wrapping_add(1).max(1);
            let tid = sched.spawn_service_process(svc.id as u8, next_gen, img.name, loaded.entry_point, loaded.stack_top, Some(loaded.address_space), caps);
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
            svc.generation.store(next_gen, Ordering::Release);
            svc.last_heartbeat.store(ticks, Ordering::Release);
            svc.state.store(ServiceState::Running as u8, Ordering::Release);
            svc.failures.store(0, Ordering::Relaxed);

            crate::println!("[SERVICE] RISC-V userspace service '{}' spawned (PID {})", img.name, tid);
            return Ok(tid);
        }
    }
    Err(())
}

pub fn stop(id: u8) -> Result<(), ()> {
    let _lock = SERVICE_LIFECYCLE_LOCK.lock();
    for svc in &SERVICES {
        if svc.id as u8 == id {
            let pid = svc.pid.load(Ordering::Acquire);
            if pid != 0 {
                let token = match ServiceLifecycleToken::issue(svc) {
                    Some(t) => t,
                    None => {
                        crate::println!("[SERVICE] Failed to issue token for PID {} during stop of '{}'", pid, svc.name);
                        return Err(());
                    }
                };
                {
                    let mut sched = crate::task::SCHEDULER.lock();
                    if let Err(e) = crate::task::terminate_service_task(&mut sched, &token) {
                        crate::println!("[SERVICE] Failed to terminate PID {} during stop of '{}': {}", pid, svc.name, e);
                        return Err(());
                    }
                }
                match crate::task::reap_service_task(&token) {
                    Ok(_) => {
                        svc.pid.store(0, Ordering::Release);
                    }
                    Err(e) => {
                        crate::println!("[SERVICE] Failed to reap PID {} during stop of '{}': {}", pid, svc.name, e);
                        return Err(());
                    }
                }
            }
            svc.state.store(ServiceState::Stopped as u8, Ordering::Release);
            return Ok(());
        }
    }
    Err(())
}

/// Publishes a heartbeat for a registered service, validating both caller PID and incarnation generation.
/// Rejects unvalidated calls (generation == 0) or incarnation mismatches to prevent zombie heartbeats.
pub fn heartbeat(caller_pid: u32, generation: u32) -> Result<(), ()> {
    if caller_pid == 0 || caller_pid == crate::task::INVALID_PID || generation == 0 {
        return Err(());
    }
    let ticks: u64;
    unsafe {
        core::arch::asm!("rdtime {0}", out(reg) ticks);
    }
    for svc in &SERVICES {
        if svc.pid.load(Ordering::Acquire) == caller_pid {
            let cur_state = svc.state.load(Ordering::Acquire);
            if cur_state == ServiceState::Stopped as u8 || cur_state == ServiceState::Crashed as u8 {
                return Err(());
            }
            let actual_gen = svc.generation.load(Ordering::Acquire);
            if actual_gen != generation {
                crate::println!("[SERVICE] Heartbeat rejected: generation mismatch for PID {} (expected {}, service has {})", caller_pid, generation, actual_gen);
                return Err(());
            }
            svc.last_heartbeat.store(ticks, Ordering::Release);
            if cur_state == ServiceState::Degraded as u8 {
                svc.state.store(ServiceState::Running as u8, Ordering::Release);
            }
            return Ok(());
        }
    }
    Err(())
}

pub fn recover_failed(id: u8) -> Result<(), ()> {
    let _lock = SERVICE_LIFECYCLE_LOCK.lock();
    for svc in &SERVICES {
        if svc.id as u8 == id {
            let st = svc.state.load(Ordering::Acquire);
            if st != ServiceState::Crashed as u8 && st != ServiceState::Degraded as u8 {
                return Err(());
            }

            let ticks: u64;
            unsafe {
                core::arch::asm!("rdtime {0}", out(reg) ticks);
            }
            let bo = svc.backoff_ticks.load(Ordering::Acquire);
            if ticks < bo {
                crate::println!("[SERVICE] Service '{}' in backoff window", svc.name);
                return Err(());
            }

            let fails = svc.failures.load(Ordering::Relaxed);
            if fails >= MAX_RESTART_ATTEMPTS {
                crate::println!("[SERVICE] Service '{}' exceeded maximum retry budget ({})", svc.name, fails);
                return Err(());
            }

            // Clean up any remaining process from previous instance
            let old_pid = svc.pid.load(Ordering::Acquire);
            if old_pid != 0 {
                let token = match ServiceLifecycleToken::issue(svc) {
                    Some(t) => t,
                    None => {
                        crate::println!("[SERVICE] Failed to issue token for recovery of PID {} for '{}'", old_pid, svc.name);
                        return Err(());
                    }
                };
                {
                    let mut sched = crate::task::SCHEDULER.lock();
                    let _ = crate::task::terminate_service_task(&mut sched, &token);
                }
                match crate::task::reap_service_task(&token) {
                    Ok(_) => {
                        svc.pid.store(0, Ordering::Release);
                    }
                    Err(e) => {
                        crate::println!("[SERVICE] Recovery reap failed for PID {}: {}", old_pid, e);
                        return Err(());
                    }
                }
            }

            svc.state.store(ServiceState::Restarting as u8, Ordering::Release);
            return match start_internal(id) {
                Ok(_) => {
                    svc.restart_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
                Err(()) => {
                    svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
                    svc.failures.fetch_add(1, Ordering::Relaxed);
                    let backoff_interval = (1u64 << fails.min(6)) * (HEARTBEAT_TIMEOUT_TICKS / 10);
                    svc.backoff_ticks.store(ticks.saturating_add(backoff_interval), Ordering::Release);
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

pub fn test_service_supervisor_lifecycle_hardening() {
    crate::println!("--- [Testing RISC-V Service Supervisor Lifecycle & Authorization Hardening] ---");
    let test_svc_id = 3; // ServiceId::Storage ("storaged")

    // In-module private test token helper for negative authorization testing
    let make_test_token = |service_id: u8, pid: u32, generation: u32| ServiceLifecycleToken {
        service_id,
        pid,
        generation,
    };

    // Clean initial state: stop if already running
    let _ = stop(test_svc_id);

    // Test 1: Start service (valid PID, registry updated, task exists with is_service: true, service_id: Some(id))
    let pid1 = start(test_svc_id).expect("Test 1: Service start must succeed");
    assert!(pid1 > 0 && !crate::task::is_reserved_pid(pid1), "Test 1: Service PID must be non-zero and non-reserved");
    {
        let sched = crate::task::SCHEDULER.lock();
        let t = sched.tasks.iter().find(|t| t.id == pid1).expect("Test 1: Service task must exist in scheduler");
        assert!(t.is_service, "Test 1: Service task must have is_service == true");
        assert_eq!(t.service_id, Some(test_svc_id), "Test 1: Service task must have service_id matching declared ID");
        assert_eq!(t.parent_id, crate::task::SUPERVISOR_PID, "Test 1: Service task must have parent_id == SUPERVISOR_PID");
    }
    assert_eq!(service_state(test_svc_id), Some(ServiceState::Running));
    assert_eq!(service_pid(test_svc_id), Some(pid1));
    crate::println!("[TEST] Test 1 (Start Service & Identity Verification): PASSED");

    // Test 2: Stop service (task removed, registry PID cleared, state Stopped)
    assert_eq!(stop(test_svc_id), Ok(()), "Test 2: Service stop must succeed");
    assert_eq!(service_state(test_svc_id), Some(ServiceState::Stopped));
    assert_eq!(service_pid(test_svc_id), None);
    {
        let sched = crate::task::SCHEDULER.lock();
        assert!(!sched.tasks.iter().any(|t| t.id == pid1), "Test 2: Stopped service task must be reaped from scheduler");
    }
    crate::println!("[TEST] Test 2 (Stop Service & Clean Reaping): PASSED");

    // Test 3: Restart service (old task reaped, new PID assigned, generation advances)
    let initial_gen = service_generation(test_svc_id).unwrap_or(0);
    let pid2 = start(test_svc_id).expect("Test 3: Service restart must succeed");
    assert!(pid2 > 0 && pid2 != pid1, "Test 3: Restarted service must receive a new distinct PID");
    let restarted_gen = service_generation(test_svc_id).unwrap_or(0);
    assert!(restarted_gen > initial_gen, "Test 3: Generation must advance on service restart");
    {
        let sched = crate::task::SCHEDULER.lock();
        let t = sched.tasks.iter().find(|t| t.id == pid2).expect("Test 3: Restarted task must exist in scheduler");
        assert!(t.is_service);
        assert_eq!(t.service_id, Some(test_svc_id));
    }
    crate::println!("[TEST] Test 3 (Restart Service & Generation Advancement): PASSED");

    // Test 4: Failed service recovery (recover_failed() on Crashed service)
    for svc in &SERVICES {
        if svc.id as u8 == test_svc_id {
            svc.state.store(ServiceState::Crashed as u8, Ordering::Release);
        }
    }
    {
        let mut sched = crate::task::SCHEDULER.lock();
        let _ = sched.terminate_task(crate::task::SUPERVISOR_PID, true, pid2);
    }
    let mut snap_buf = [0u8; 1024];
    let _ = snapshot(&mut snap_buf);
    assert_eq!(recover_failed(test_svc_id), Ok(()), "Test 4: recover_failed must succeed");
    let pid3 = service_pid(test_svc_id).expect("Test 4: Recovered service must have active PID");
    assert_eq!(service_state(test_svc_id), Some(ServiceState::Running));
    assert!(pid3 > 0 && pid3 != pid2, "Test 4: Recovered service must have new PID");
    {
        let sched = crate::task::SCHEDULER.lock();
        assert!(!sched.tasks.iter().any(|t| t.id == pid2), "Test 4: Old crashed PID must be reaped");
        let t = sched.tasks.iter().find(|t| t.id == pid3).expect("Test 4: New service task must be active");
        assert!(t.is_service);
    }
    crate::println!("[TEST] Test 4 (Failed Service Recovery & Reaping): PASSED");

    // Test 5: Unauthorized ordinary user process rejection with -EPERM
    let user_pid = {
        let mut sched = crate::task::SCHEDULER.lock();
        sched.spawn_user_process(1, "unauth_user", 0x1000, 0x2000, None, crate::capability::CapabilityTable::new())
    };
    assert!(user_pid > 0);
    let forged_token = make_test_token(test_svc_id, user_pid, 1);
    assert_eq!(crate::task::reap_service_task(&forged_token), Err(-1), "Test 5: reap_service_task on ordinary user process must fail with -EPERM (-1)");
    {
        let mut sched = crate::task::SCHEDULER.lock();
        assert_eq!(crate::task::terminate_service_task(&mut sched, &forged_token), Err(-1), "Test 5: terminate_service_task on ordinary user process must fail with -EPERM (-1)");
        let pos = sched.tasks.iter().position(|t| t.id == user_pid).unwrap();
        sched.remove_task_at(pos);
    }
    crate::println!("[TEST] Test 5 (Unauthorized Ordinary Process Rejection -EPERM): PASSED");

    // Test 6: Stale PID / generation token rejection with -EPERM
    let cur_gen = service_generation(test_svc_id).expect("service generation");
    let stale_gen_token = make_test_token(test_svc_id, pid3, cur_gen.wrapping_add(999));
    assert_eq!(crate::task::reap_service_task(&stale_gen_token), Err(-1), "Test 6: Token with stale generation must be rejected with -EPERM (-1)");
    {
        let mut sched = crate::task::SCHEDULER.lock();
        assert_eq!(crate::task::terminate_service_task(&mut sched, &stale_gen_token), Err(-1), "Test 6: terminate with stale generation must fail with -EPERM (-1)");
    }
    let stale_pid_token = make_test_token(test_svc_id, 0x9999, cur_gen);
    assert_eq!(crate::task::reap_service_task(&stale_pid_token), Err(-1), "Test 6: Token with stale PID must be rejected with -EPERM (-1)");
    crate::println!("[TEST] Test 6 (Stale PID & Generation Token Rejection -EPERM): PASSED");

    // Test 7: Service registration mismatch rejection with -EPERM (-1)
    let mismatch_token = make_test_token(test_svc_id + 1, pid3, cur_gen);
    assert_eq!(crate::task::reap_service_task(&mismatch_token), Err(-1), "Test 7: Token with mismatched service ID must fail with -EPERM (-1)");
    {
        let mut sched = crate::task::SCHEDULER.lock();
        assert_eq!(crate::task::terminate_service_task(&mut sched, &mismatch_token), Err(-1), "Test 7: terminate with mismatched service ID must fail with -EPERM (-1)");
    }
    crate::println!("[TEST] Test 7 (Service Registration Mismatch Rejection -EPERM): PASSED");

    // Test 8: Reserved PID registration & spawn_service_process validation
    {
        let mut sched = crate::task::SCHEDULER.lock();
        assert!(!sched.add_task(crate::task::TaskId(0), 0, 0, 0, 0), "Test 8: Explicit registration of PID 0 must be rejected");
        assert!(!sched.add_task(crate::task::TaskId(crate::task::SUPERVISOR_PID as usize), 0, 0, 0, 0), "Test 8: Explicit registration of SUPERVISOR_PID must be rejected");
        assert!(!sched.add_task(crate::task::TaskId(crate::task::INVALID_PID as usize), 0, 0, 0, 0), "Test 8: Explicit registration of INVALID_PID must be rejected");
        let reg_ok = sched.add_task(crate::task::TaskId(23456), 0, 0, 0, 0);
        assert!(reg_ok, "Test 8: Explicit registration of valid PID must succeed");
        let reg_dup = sched.add_task(crate::task::TaskId(23456), 0, 0, 0, 0);
        assert!(!reg_dup, "Test 8: Duplicate PID registration must be rejected");
        if let Some(pos) = sched.tasks.iter().position(|t| t.id == 23456) {
            sched.remove_task_at(pos);
        }

        // Validate spawn_service_process generation and ID validation
        assert_eq!(sched.spawn_service_process(test_svc_id, 0, "gen0_svc", 0x1000, 0x2000, None, crate::capability::CapabilityTable::new()), 0, "Test 8: spawn_service_process with generation 0 must be rejected");
        assert_eq!(sched.spawn_service_process(255, 1, "inv_svc", 0x1000, 0x2000, None, crate::capability::CapabilityTable::new()), 0, "Test 8: spawn_service_process with invalid service ID must be rejected");
        assert!(is_valid_service_id(test_svc_id), "Test 8: test_svc_id must be valid");
        assert!(!is_valid_service_id(255), "Test 8: service_id 255 must be invalid");
    }
    crate::println!("[TEST] Test 8 (Reserved PID Registration Rejection & Spawn Validation): PASSED");

    // Test 9: Failure paths without silently clearing service PID
    {
        let recorded_pid = service_pid(test_svc_id).expect("Service PID must be active");
        assert_eq!(recorded_pid, pid3);
        // An unauthenticated / invalid token reap does NOT clear the registry PID
        let invalid_token = make_test_token(test_svc_id, pid3, cur_gen.wrapping_add(555));
        let reap_fail = crate::task::reap_service_task(&invalid_token);
        assert_eq!(reap_fail, Err(-1));
        assert_eq!(service_pid(test_svc_id), Some(pid3), "Test 9: Failed reap must NOT clear service PID from registry");
    }
    crate::println!("[TEST] Test 9 (Failure Paths Preserve Service PID): PASSED");

    // Test 10: Verify process absence and replacement integrity
    assert_eq!(stop(test_svc_id), Ok(()));
    {
        let sched = crate::task::SCHEDULER.lock();
        assert!(!sched.tasks.iter().any(|t| t.id == pid1), "Test 10: pid1 must be absent");
        assert!(!sched.tasks.iter().any(|t| t.id == pid2), "Test 10: pid2 must be absent");
        assert!(!sched.tasks.iter().any(|t| t.id == pid3), "Test 10: pid3 must be reaped after stop");
    }
    assert_eq!(service_pid(test_svc_id), None, "Test 10: Stopped service PID must be None");
    crate::println!("[TEST] Test 10 (Process Absence & Replacement Integrity): PASSED");
    crate::println!("[TEST] RISC-V Service Supervisor Lifecycle Hardening 10/10 Tests VERIFIED.");
}
