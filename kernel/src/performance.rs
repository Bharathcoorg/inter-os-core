//! Low-overhead performance measurement primitives.
//!
//! Metrics are intentionally architecture-neutral so the same contracts can
//! be consumed by QEMU, hardware backends, and developer tooling.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counter {
    pub samples: u64,
    pub total_ticks: u64,
    pub worst_ticks: u64,
}

impl Counter {
    pub const fn new() -> Self { Self { samples: 0, total_ticks: 0, worst_ticks: 0 } }

    pub fn observe(&mut self, ticks: u64) {
        self.samples = self.samples.saturating_add(1);
        self.total_ticks = self.total_ticks.saturating_add(ticks);
        self.worst_ticks = self.worst_ticks.max(ticks);
    }

    pub const fn average_ticks(&self) -> u64 {
        if self.samples == 0 { 0 } else { self.total_ticks / self.samples }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerformanceSnapshot {
    pub scheduler: Counter,
    pub ipc: Counter,
    pub frame: Counter,
}

impl PerformanceSnapshot {
    pub const fn new() -> Self {
        Self { scheduler: Counter::new(), ipc: Counter::new(), frame: Counter::new() }
    }
}


use spin::Mutex;
pub static PERFORMANCE: Mutex<PerformanceSnapshot> = Mutex::new(PerformanceSnapshot::new());

pub fn init() { *PERFORMANCE.lock() = PerformanceSnapshot::new(); }
