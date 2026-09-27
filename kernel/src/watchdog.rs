//! Bounded service watchdog policy.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogState { Disabled, Armed, Expired }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watchdog { pub state: WatchdogState, pub deadline_ticks: u64, pub last_kick_ticks: u64, pub timeout_ticks: u64 }

impl Watchdog {
    pub const fn new(timeout_ticks: u64) -> Self { Self { state: WatchdogState::Disabled, deadline_ticks: 0, last_kick_ticks: 0, timeout_ticks } }
    pub fn arm(&mut self, now: u64) { self.state=WatchdogState::Armed; self.last_kick_ticks=now; self.deadline_ticks=now.saturating_add(self.timeout_ticks); }
    pub fn kick(&mut self, now: u64) { if self.state==WatchdogState::Armed { self.last_kick_ticks=now; self.deadline_ticks=now.saturating_add(self.timeout_ticks); } }
    pub fn poll(&mut self, now: u64) -> bool { if self.state==WatchdogState::Armed && now>=self.deadline_ticks { self.state=WatchdogState::Expired; true } else { false } }
    pub fn disarm(&mut self) { self.state=WatchdogState::Disabled; }
}

#[cfg(test)]
mod tests { use super::*; #[test] fn expires_at_deadline() { let mut w=Watchdog::new(10); w.arm(100); assert!(!w.poll(109)); assert!(w.poll(110)); assert_eq!(w.state,WatchdogState::Expired); } }
