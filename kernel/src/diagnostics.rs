// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Structured kernel diagnostics.
//!
//! Provides bounded, allocation-free event storage suitable for boot and
//! failure analysis. Consumers can snapshot records without exposing mutable
//! internal state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DiagnosticLevel { Info = 0, Warning = 1, Error = 2, Critical = 3 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticEvent {
    pub sequence: u64,
    pub timestamp_ticks: u64,
    pub level: DiagnosticLevel,
    pub code: u16,
    pub argument: u64,
}

const CAPACITY: usize = 64;

pub struct DiagnosticRing {
    entries: [Option<DiagnosticEvent>; CAPACITY],
    next: usize,
    sequence: u64,
}

impl DiagnosticRing {
    pub const fn new() -> Self {
        Self { entries: [None; CAPACITY], next: 0, sequence: 0 }
    }

    pub fn record(&mut self, timestamp_ticks: u64, level: DiagnosticLevel, code: u16, argument: u64) {
        let event = DiagnosticEvent {
            sequence: self.sequence,
            timestamp_ticks,
            level,
            code,
            argument,
        };
        self.sequence = self.sequence.wrapping_add(1);
        self.entries[self.next] = Some(event);
        self.next = (self.next + 1) % CAPACITY;
    }

    pub fn latest(&self) -> Option<DiagnosticEvent> {
        let idx = if self.next == 0 { CAPACITY - 1 } else { self.next - 1 };
        self.entries[idx]
    }

    pub fn snapshot(&self, out: &mut [DiagnosticEvent]) -> usize {
        let count = out.len().min(CAPACITY);
        let available = self.entries.iter().filter(|e| e.is_some()).count();
        let take = count.min(available);
        let start_seq = self.sequence.saturating_sub(take as u64);
        let mut written = 0;
        for e in self.entries.iter().flatten() {
            if e.sequence >= start_seq && written < take {
                out[written] = *e;
                written += 1;
            }
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ring_is_bounded_and_keeps_latest_events() {
        let mut d = DiagnosticRing::new();
        for i in 0..70 { d.record(i, DiagnosticLevel::Info, 7, i); }
        assert_eq!(d.latest().unwrap().argument, 69);
        let mut out = [DiagnosticEvent { sequence: 0, timestamp_ticks: 0, level: DiagnosticLevel::Info, code: 0, argument: 0 }; 4];
        assert_eq!(d.snapshot(&mut out), 4);
        assert_eq!(out[3].argument, 69);
    }
}

use spin::Mutex;
pub static DIAGNOSTICS: Mutex<DiagnosticRing> = Mutex::new(DiagnosticRing::new());

pub fn init() { DIAGNOSTICS.lock().record(0, DiagnosticLevel::Info, 1, 0); }
