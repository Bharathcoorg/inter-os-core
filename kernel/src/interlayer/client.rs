//! InterLayer Sovereign Client Node
//!
//! Manages offline-first state synchronization, remote verification tunnels,
//! and cryptographic DID handshakes over UDP network sockets.

use super::protocol::{InterlayerPacket, PacketType};
use crate::capability::CapabilityTable;
use crate::identity::IDENTITY_VAULT;
use crate::net::{send_udp_datagram, Ipv4Address, NetError};

pub const INTERLAYER_DEFAULT_PORT: u16 = 8443;

/// In-Kernel Sovereign InterLayer Client Instance
pub struct InterlayerClient {
    pub is_connected: bool,
    pub tx_sequence: u32,
    pub rx_sequence: u32,
    pub state_syncs_transmitted: u64,
}

impl InterlayerClient {
    pub const fn new() -> Self {
        Self {
            is_connected: false,
            tx_sequence: 1,
            rx_sequence: 0,
            state_syncs_transmitted: 0,
        }
    }

    /// Generates and transmits a signed State Delta Synchronization packet to a peer.
    pub fn sync_state_delta(
        &mut self,
        peer_ip: Ipv4Address,
        delta_root: [u8; 32],
        caps: &CapabilityTable,
    ) -> Result<(), NetError> {
        let vault_guard = IDENTITY_VAULT.lock();
        let vault = vault_guard.as_ref().ok_or(NetError::DeviceDown)?;
        let did = vault.device_did();
        let master_secret = b"INTER_OS_MASTER_SEED_ZERO_TRUST_DEV_KEY";

        let mut pkt_buf = [0u8; 256];
        let pkt_len = InterlayerPacket::serialize_signed(
            PacketType::StateDeltaSync,
            self.tx_sequence,
            *did.fingerprint(),
            &delta_root,
            master_secret,
            &mut pkt_buf,
        ).map_err(|_| NetError::BufferTooSmall)?;

        self.tx_sequence += 1;
        self.state_syncs_transmitted += 1;

        // Transmit via In-Kernel Network Stack
        send_udp_datagram(
            Ipv4Address::LOCALHOST,
            INTERLAYER_DEFAULT_PORT,
            peer_ip,
            INTERLAYER_DEFAULT_PORT,
            &pkt_buf[0..pkt_len],
            caps,
        )
    }

    /// Transmits an attested automation intent relay packet to a remote peer.
    pub fn relay_attested_intent(
        &mut self,
        peer_ip: Ipv4Address,
        intent: &'static str,
        caps: &CapabilityTable,
    ) -> Result<(), NetError> {
        let vault_guard = IDENTITY_VAULT.lock();
        let vault = vault_guard.as_ref().ok_or(NetError::DeviceDown)?;
        let did = vault.device_did();
        let master_secret = b"INTER_OS_MASTER_SEED_ZERO_TRUST_DEV_KEY";

        let mut pkt_buf = [0u8; 256];
        let pkt_len = InterlayerPacket::serialize_signed(
            PacketType::AttestedIntentRelay,
            self.tx_sequence,
            *did.fingerprint(),
            intent.as_bytes(),
            master_secret,
            &mut pkt_buf,
        ).map_err(|_| NetError::BufferTooSmall)?;

        self.tx_sequence += 1;

        send_udp_datagram(
            Ipv4Address::LOCALHOST,
            INTERLAYER_DEFAULT_PORT,
            peer_ip,
            INTERLAYER_DEFAULT_PORT,
            &pkt_buf[0..pkt_len],
            caps,
        )
    }
}
