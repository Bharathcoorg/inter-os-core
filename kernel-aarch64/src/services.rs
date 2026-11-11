// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Authoritative Service Registry & Lifecycle Manager
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
    Declared = 0,
    Starting = 1,
    Running = 2,
    Stopped = 3,
    Failed = 4,
}

pub struct ServiceEntry {
    pub id: ServiceId,
    pub name: &'static str,
    pub state: AtomicU8,
    pub restart: u8,
    pub critical: u8,
    pub generation: AtomicU32,
    pub last_heartbeat: AtomicU64,
    pub failures: AtomicU32,
}

// 12 core declared services matching userspace/init discovery expectations
pub static SERVICES: [ServiceEntry; 12] = [
    ServiceEntry { id: ServiceId::Display, name: "displayd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Input, name: "inputd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Storage, name: "storaged", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Network, name: "netd", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Power, name: "powerd", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Modem, name: "modemd", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Audio, name: "audiod", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Camera, name: "camerad", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Compositor, name: "compositor", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Launcher, name: "launcher", state: AtomicU8::new(0), restart: 1, critical: 1, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Sensor, name: "sensord", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
    ServiceEntry { id: ServiceId::Inference, name: "aid", state: AtomicU8::new(0), restart: 1, critical: 0, generation: AtomicU32::new(1), last_heartbeat: AtomicU64::new(0), failures: AtomicU32::new(0) },
];

pub fn start(id: u8) -> Result<(), ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            svc.state.store(ServiceState::Running as u8, Ordering::Release);
            svc.generation.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }
    Err(())
}

pub fn stop(id: u8) -> Result<(), ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            svc.state.store(ServiceState::Stopped as u8, Ordering::Release);
            return Ok(());
        }
    }
    Err(())
}

pub fn recover_failed(id: u8) -> Result<(), ()> {
    for svc in &SERVICES {
        if svc.id as u8 == id {
            svc.failures.fetch_add(1, Ordering::Relaxed);
            svc.state.store(ServiceState::Running as u8, Ordering::Release);
            svc.generation.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }
    Err(())
}

/// Takes a snapshot of all services in pipe-separated format:
/// `{id}|1|{state_str}|{restart}|{critical}|{generation}|{age}|{failures}\n`
pub fn snapshot(out: &mut [u8]) -> usize {
    use core::fmt::Write;

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
    for svc in &SERVICES {
        let state_val = svc.state.load(Ordering::Acquire);
        let state_str = match state_val {
            0 => "declared",
            1 => "starting",
            2 => "running",
            3 => "stopped",
            _ => "failed",
        };
        let gen = svc.generation.load(Ordering::Relaxed);
        let fails = svc.failures.load(Ordering::Relaxed);
        let _ = writeln!(
            writer,
            "{}|1|{}|{}|{}|{}|0|{}",
            svc.id as u8, state_str, svc.restart, svc.critical, gen, fails
        );
    }
    writer.pos
}
