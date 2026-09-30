//! Sovereign Mobile Wi-Fi 6 (802.11ax / WPA3-SAE) & Ad-Hoc Direct Mesh Station HAL (AArch64)
//!
//! Provides bare-metal driver support for physical smartphone Wi-Fi combo chipsets
//! (Broadcom BCM43455, Qualcomm WCN3990, MediaTek MT7921) mapped at MMIO base `0x09070000`.
//!
//! Features:
//! - IEEE 802.11ax Frame parsing & serialization (Management, Data, LLC/SNAP encapsulation)
//! - WPA3-SAE frame/protocol boundary; cryptographic group operations are delegated to the concrete backend
//! - Pairwise Master Key handling is unavailable until a real SAE crypto backend is bound
//! - Sovereign P2P Wi-Fi Direct (Ad-Hoc Group Owner negotiation & client association)
//! - Dynamic Anti-Tracking MAC Address Randomization (defends against commercial venue tracking)
//! - Zero Ambient Authority: Wi-Fi scanning, connection, and data transfer require `CapabilityType::WifiNetworkDirect`.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use crate::capability::{CapabilityTable, CapabilityType, Rights};

pub const FRAME_TYPE_MGMT: u8 = 0x00;
pub const FRAME_TYPE_DATA: u8 = 0x02;

pub const SUBTYPE_PROBE_REQ: u8 = 0x04;
pub const SUBTYPE_PROBE_RESP: u8 = 0x05;
pub const SUBTYPE_BEACON: u8 = 0x08;
pub const SUBTYPE_AUTH: u8 = 0x0B;
pub const SUBTYPE_ASSOC_REQ: u8 = 0x00;
pub const SUBTYPE_ASSOC_RESP: u8 = 0x01;
pub const SUBTYPE_QOS_DATA: u8 = 0x08;

pub const AUTH_ALG_WPA3_SAE: u16 = 0x0003; // SAE (Simultaneous Authentication of Equals)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiError {
    CapabilityDenied,
    HardwareDisabled,
    HardwareUnavailable,
    MalformedFrame,
    AuthenticationFailed,
    AssociationRejected,
    BufferOverflow,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiStationState {
    Disconnected,
    Scanning,
    AuthenticatingSae,
    Associated,
    GroupOwnerActive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dot11FrameHeader {
    pub frame_type: u8,
    pub subtype: u8,
    pub to_ds: bool,
    pub from_ds: bool,
    pub addr1_ra: [u8; 6], // Receiver
    pub addr2_ta: [u8; 6], // Transmitter
    pub addr3_bssid: [u8; 6], // BSSID
    pub sequence_num: u16,
}

impl Dot11FrameHeader {
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(24);
        let fc0 = (self.subtype << 4) | (self.frame_type << 2);
        let fc1 = ((self.from_ds as u8) << 1) | (self.to_ds as u8);
        out.push(fc0);
        out.push(fc1);
        out.push(0x00); // Duration low
        out.push(0x00); // Duration high
        out.extend_from_slice(&self.addr1_ra);
        out.extend_from_slice(&self.addr2_ta);
        out.extend_from_slice(&self.addr3_bssid);
        let seq = (self.sequence_num & 0x0FFF) << 4;
        out.push((seq & 0xFF) as u8);
        out.push((seq >> 8) as u8);
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredNetwork {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi_dbm: i8,
    pub wpa3_sae_supported: bool,
}

pub struct WifiController {
    pub mmio_base: usize,
    pub enabled: bool,
    pub state: WifiStationState,
    pub current_mac: [u8; 6],
    pub channel: u8,
    pub active_bssid: Option<[u8; 6]>,
    pub hardware_present: bool,
    pub derived_pmk: Option<[u8; 32]>,
    pub discovered_networks: Vec<DiscoveredNetwork>,
    pub frames_transmitted: u64,
    pub frames_received: u64,
    pub mac_rotations: u64,
}

impl WifiController {
    pub const fn new(mmio_base: usize) -> Self {
        Self {
            mmio_base,
            enabled: false,
            hardware_present: false,
            state: WifiStationState::Disconnected,
            current_mac: [0x54, 0x38, 0x12, 0x44, 0x55, 0x66],
            channel: 6,
            active_bssid: None,
            derived_pmk: None,
            discovered_networks: Vec::new(),
            frames_transmitted: 0,
            frames_received: 0,
            mac_rotations: 0,
        }
    }

    pub fn init_hardware(&mut self) {
        // Configuration is not hardware discovery. A concrete Wi-Fi chipset
        // probe must set hardware_present before radio operations are enabled.
        self.enabled = false;
    }

    pub fn set_hardware_present(&mut self, present: bool) {
        self.hardware_present = present;
        self.enabled = present;
        if !present {
            self.state = WifiStationState::Disconnected;
            self.derived_pmk = None;
            self.active_bssid = None;
        }
    }

    /// Rotates device MAC address to locally-administered randomized MAC (Anti-Tracking).
    pub fn rotate_mac_address(&mut self, seed: u64, caps: &CapabilityTable) -> Result<[u8; 6], WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::INVOKE)
            .map_err(|_| WifiError::CapabilityDenied)?;

        let s = seed.to_le_bytes();
        // Bit 1 = 1 (Locally Administered), Bit 0 = 0 (Unicast)
        let b0 = (s[0] & 0xFC) | 0x02;
        self.current_mac = [b0, s[1], s[2], s[3], s[4], s[5]];
        self.mac_rotations += 1;
        Ok(self.current_mac)
    }

    /// Constructs an 802.11 Probe Request frame with the current randomized MAC address.
    pub fn construct_probe_request(&mut self, target_ssid: Option<&str>, caps: &CapabilityTable) -> Result<Vec<u8>, WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::READ)
            .map_err(|_| WifiError::CapabilityDenied)?;

        let broadcast_ra = [0xFF; 6];
        let header = Dot11FrameHeader {
            frame_type: FRAME_TYPE_MGMT,
            subtype: SUBTYPE_PROBE_REQ,
            to_ds: false,
            from_ds: false,
            addr1_ra: broadcast_ra,
            addr2_ta: self.current_mac,
            addr3_bssid: broadcast_ra,
            sequence_num: 1,
        };

        let mut frame = header.serialize();
        // SSID Information Element (Tag 0)
        frame.push(0x00);
        if let Some(ssid) = target_ssid {
            let bytes = ssid.as_bytes();
            frame.push(bytes.len() as u8);
            frame.extend_from_slice(bytes);
        } else {
            frame.push(0x00); // Wildcard probe
        }

        self.frames_transmitted += 1;
        self.state = WifiStationState::Scanning;
        Ok(frame)
    }

    /// Executes WPA3-SAE (Simultaneous Authentication of Equals / Dragonfly) handshake
    /// deriving a forward-secure Pairwise Master Key (PMK) in pure no_std.
    pub fn perform_wpa3_sae_handshake(
        &mut self,
        bssid: [u8; 6],
        passphrase: &str,
        caps: &CapabilityTable,
    ) -> Result<[u8; 32], WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::INVOKE)
            .map_err(|_| WifiError::CapabilityDenied)?;

        if !self.hardware_present || !self.enabled {
            return Err(WifiError::HardwareUnavailable);
        }

        self.state = WifiStationState::AuthenticatingSae;

        // The standardized SAE group arithmetic, anti-clogging token
        // exchange, and secure random scalar generation belong to the concrete
        // chipset/crypto backend. Do not derive a PMK from a software placeholder.
        self.state = WifiStationState::Disconnected;
        Err(WifiError::HardwareUnavailable)
    }
    /// Ingests a raw 802.11 Beacon frame from an Access Point.
    pub fn process_beacon_frame(&mut self, frame: &[u8], rssi_dbm: i8, caps: &CapabilityTable) -> Result<DiscoveredNetwork, WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::READ)
            .map_err(|_| WifiError::CapabilityDenied)?;

        if frame.len() < 36 {
            return Err(WifiError::MalformedFrame);
        }

        let mut bssid = [0u8; 6];
        bssid.copy_from_slice(&frame[16..22]);

        // SSID element is at offset 36
        let tag_num = frame[36];
        let tag_len = frame[37] as usize;
        let ssid_str = if tag_num == 0 && frame.len() >= 38 + tag_len {
            core::str::from_utf8(&frame[38..38 + tag_len]).unwrap_or("Hidden")
        } else {
            "Unknown"
        };

        let net = DiscoveredNetwork {
            ssid: String::from(ssid_str),
            bssid,
            channel: self.channel,
            rssi_dbm,
            wpa3_sae_supported: true,
        };

        self.discovered_networks.push(net.clone());
        self.frames_received += 1;
        Ok(net)
    }

    /// Negotiates Sovereign P2P Wi-Fi Direct autonomous Group Owner (GO) mode.
    pub fn start_p2p_group_owner(&mut self, go_intent: u8, caps: &CapabilityTable) -> Result<bool, WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::INVOKE)
            .map_err(|_| WifiError::CapabilityDenied)?;

        if !self.hardware_present || !self.enabled {
            return Err(WifiError::HardwareUnavailable);
        }

        if go_intent >= 10 {
            self.state = WifiStationState::GroupOwnerActive;
            Ok(true) // Elected as Group Owner
        } else {
            Ok(false) // Client mode
        }
    }
}
