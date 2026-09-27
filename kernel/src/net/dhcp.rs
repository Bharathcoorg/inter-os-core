//! DHCP lease state for network interfaces.
//!
//! Packet transport remains below this layer; this module only models bounded
//! lease acquisition/renewal and avoids advertising an address before a valid
//! lease exists.

use super::ipv4::Ipv4Address;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DhcpState { Down, Discovering, Requesting, Bound, Renewing, Expired }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DhcpLease {
    pub address: Ipv4Address,
    pub gateway: Ipv4Address,
    pub dns: Ipv4Address,
    pub lease_seconds: u32,
    pub acquired_at: u64,
}

pub struct DhcpClient {
    pub state: DhcpState,
    pub lease: Option<DhcpLease>,
}

impl DhcpClient {
    pub const fn new() -> Self { Self { state: DhcpState::Down, lease: None } }

    pub fn start(&mut self) { self.lease = None; self.state = DhcpState::Discovering; }

    pub fn offer(&mut self) {
        if self.state == DhcpState::Discovering { self.state = DhcpState::Requesting; }
    }

    pub fn bind(&mut self, lease: DhcpLease) -> Result<(), &'static str> {
        if lease.lease_seconds == 0 { return Err("DHCP lease duration is zero"); }
        self.lease = Some(lease);
        self.state = DhcpState::Bound;
        Ok(())
    }

    pub fn tick(&mut self, now: u64) {
        let Some(lease) = self.lease else { return; };
        if now.saturating_sub(lease.acquired_at) >= lease.lease_seconds as u64 {
            self.state = DhcpState::Expired;
            self.lease = None;
        } else if now.saturating_sub(lease.acquired_at) >= (lease.lease_seconds as u64 / 2).max(1) {
            self.state = DhcpState::Renewing;
        }
    }

    pub fn address(&self) -> Option<Ipv4Address> {
        if self.state == DhcpState::Bound || self.state == DhcpState::Renewing {
            self.lease.map(|l| l.address)
        } else { None }
    }
}
