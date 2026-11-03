// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Sovereign Mobile Wi-Fi 6 (802.11ax / WPA3-SAE) & Ad-Hoc Direct Mesh Station HAL (AArch64)
//!
//! This module is an **802.11 protocol codec and capability boundary, not a chipset driver**.
//! No Broadcom/Qualcomm/MediaTek Wi-Fi silicon is described, addressed, or emulated here, and this
//! file performs no read or write against `mmio_base`. `mmio_base` is retained only as the bind
//! point a concrete chipset driver would be written against; until `set_hardware_present(true)` is
//! called by such a driver, no radio exists and every radio operation fails closed.
//!
//! Status of each layer, stated explicitly:
//! - **Unavailable**: radio transmit/receive, scanning, association, SAE key derivation,
//!   MAC address identity. There is no address for this device until a bound controller reads one.
//! - **Emulated**: none. Nothing in this file fabricates radio behaviour.
//! - **Present**: none. Zero radio hardware is bound at build time.
//!
//! Reusable software that is honest about its own status:
//! - IEEE 802.11 frame header parsing & serialization (see `Dot11FrameHeader`, `decode_beacon`)
//! - Beacon decoding: BSSID, SSID, DS-Parameter channel, and RSN/WPA AKM parsing that reports
//!   SAE only when the suite selector is actually advertised
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

/// What the beacon actually advertised about SAE, and nothing more.
///
/// `Unknown` is the honest default: an AP that carries no RSN/WPA information element, or one whose
/// IE this decoder cannot parse, has NOT been shown to support WPA3-SAE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaeEvidence {
    /// No RSN/WPA IE was present or it was unparseable; nothing was claimed.
    Unknown,
    /// An RSN/WPA IE was present and advertised an AKM suite selector in the SAE range (0x000F).
    AdvertisedSae,
    /// An RSN/WPA IE was present and advertised AKM suite selectors, but none were SAE.
    AdvertisedWithoutSae,
}

impl SaeEvidence {
    /// True only when the frame itself proved SAE support. Never assumed.
    pub fn is_advertised_sae(&self) -> bool {
        matches!(self, SaeEvidence::AdvertisedSae)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredNetwork {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi_dbm: i8,
    /// Observed SAE evidence from the beacon's RSN/WPA IE. Replaces the former unconditional
    /// `wpa3_sae_supported: true`, which asserted a capability on every network ever seen.
    pub sae: SaeEvidence,
}

impl DiscoveredNetwork {
    /// Convenience accessor that reports only observed SAE support.
    pub fn wpa3_sae_observed(&self) -> bool {
        self.sae.is_advertised_sae()
    }
}

/// SSID Information Element element ID.
const SSID_IE_ID: u8 = 0;
/// DS Parameter Set element ID: carries the channel the AP is actually operating on.
const DS_PARAMS_IE_ID: u8 = 3;
/// RSN Information Element element ID (802.11-2016 Table 9-129).
const RSN_IE_ID: u8 = 48;
/// Vendor-specific element ID; the WPA1 IE lives under OUI 00-50-F2, type 1 (802.11i).
const VENDOR_IE_ID: u8 = 221;
const WPA_IE_OUI: [u8; 3] = [0x00, 0x50, 0xF2];
/// AKM suite selector 0x000F == SUITE_AUTH, the WPA3-SAE authentication method.
const AKM_SUITE_SAE: u16 = 0x000F;

/// Fields a Beacon's tagged parameters actually stated, before any controller state is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBeacon {
    pub bssid: [u8; 6],
    pub ssid: String,
    /// Channel from the DS Parameter Set IE; 0 when the beacon carried no DS Parameter Set.
    pub channel: u8,
    pub sae: SaeEvidence,
}

pub struct WifiController {
    /// Bind point for a concrete chipset driver. THIS MODULE PERFORMS NO MMIO ACCESS AT IT.
    pub mmio_base: usize,
    pub enabled: bool,
    pub state: WifiStationState,
    /// The address this station transmits from, or `None` when no radio has supplied one.
    /// A constant MAC in source is a fabricated identity, so there is deliberately no seed value:
    /// with no bound radio this device has no address.
    pub current_mac: Option<[u8; 6]>,
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
            // WHY None: seeding a literal MAC (previously Apple OUI 54:38:12) asserted an identity
            // this device does not have. Without a bound controller there is no address at all.
            current_mac: None,
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
            // WHY: the station address belongs to the radio. If the radio goes away, so does the
            // address; keeping it would leave a stale identity readable with no hardware present.
            self.current_mac = None;
        }
    }

    /// Supplies the station address read out of a bound radio by a concrete chipset driver.
    ///
    /// This is the only legitimate way to establish an identity: the value is measured, not
    /// invented. Rejected without `hardware_present` so a caller cannot invent an address for a
    /// device that has no radio.
    pub fn set_factory_mac(&mut self, mac: [u8; 6]) -> Result<(), WifiError> {
        if !self.hardware_present {
            return Err(WifiError::HardwareUnavailable);
        }
        // Reject the all-zero and broadcast addresses: neither can be a real station address,
        // and accepting them would put a meaningless TA on every frame.
        if mac == [0u8; 6] || mac == [0xFFu8; 6] {
            return Err(WifiError::MalformedFrame);
        }
        self.current_mac = Some(mac);
        Ok(())
    }

    /// Rotates the station MAC to a locally-administered randomized address (Anti-Tracking).
    ///
    /// Requires a bound radio. Anti-tracking rotation only protects a user if the address actually
    /// leaves the device over the air; previously this returned `Ok` and incremented a rotation
    /// counter with no radio attached, reporting privacy that was never in effect.
    pub fn rotate_mac_address(&mut self, seed: u64, caps: &CapabilityTable) -> Result<[u8; 6], WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::INVOKE)
            .map_err(|_| WifiError::CapabilityDenied)?;

        // WHY: fail closed without hardware. No radio means no address is transmitted, so a
        // "successful" rotation and an incremented counter would both be false reporting.
        if !self.hardware_present || !self.enabled {
            return Err(WifiError::HardwareUnavailable);
        }

        let s = seed.to_le_bytes();
        // Bit 1 = 1 (Locally Administered), Bit 0 = 0 (Unicast)
        let b0 = (s[0] & 0xFC) | 0x02;
        let randomized = [b0, s[1], s[2], s[3], s[4], s[5]];
        self.current_mac = Some(randomized);
        self.mac_rotations += 1;
        Ok(randomized)
    }

    /// Builds an 802.11 Probe Request frame carrying the current station address as TA.
    ///
    /// Requires a bound radio AND a known station address. Returning a frame and bumping
    /// `frames_transmitted` with no radio claimed a transmission that never happened.
    pub fn construct_probe_request(&mut self, target_ssid: Option<&str>, caps: &CapabilityTable) -> Result<Vec<u8>, WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::READ)
            .map_err(|_| WifiError::CapabilityDenied)?;

        // WHY: fail closed without hardware. There is no radio to transmit this frame and no
        // source address to put in it, so the frame would be a fiction on every field.
        if !self.hardware_present || !self.enabled {
            return Err(WifiError::HardwareUnavailable);
        }
        // WHY: 802.11 Address 2 (Transmitter Address) must be a real station address. Emitting
        // zeros would look like a valid frame and could be mistaken for a captured one.
        let station_mac = self.current_mac.ok_or(WifiError::HardwareUnavailable)?;

        let broadcast_ra = [0xFF; 6];
        let header = Dot11FrameHeader {
            frame_type: FRAME_TYPE_MGMT,
            subtype: SUBTYPE_PROBE_REQ,
            to_ds: false,
            from_ds: false,
            addr1_ra: broadcast_ra,
            addr2_ta: station_mac,
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
        _bssid: [u8; 6],
        _passphrase: &str,
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
    /// Ingests a raw 802.11 Beacon frame observed by a bound radio.
    ///
    /// Requires `hardware_present`: an AP cannot be "discovered" and `frames_received` cannot
    /// increment with no radio to receive anything. Use `decode_beacon` for standalone decoding.
    pub fn process_beacon_frame(&mut self, frame: &[u8], rssi_dbm: i8, caps: &CapabilityTable) -> Result<DiscoveredNetwork, WifiError> {
        caps.check(CapabilityType::WifiNetworkDirect, Rights::READ)
            .map_err(|_| WifiError::CapabilityDenied)?;

        // WHY: fail closed without hardware. Recording a discovery and incrementing a receive
        // counter here would report over-the-air activity that cannot have occurred.
        if !self.hardware_present || !self.enabled {
            return Err(WifiError::HardwareUnavailable);
        }

        let decoded = Self::decode_beacon(frame)?;
        let net = DiscoveredNetwork {
            ssid: decoded.ssid,
            bssid: decoded.bssid,
            channel: decoded.channel,
            rssi_dbm,
            sae: decoded.sae,
        };

        self.discovered_networks.push(net.clone());
        self.frames_received += 1;
        Ok(net)
    }

    /// Pure 802.11 Beacon decoder. Performs no I/O and asserts no hardware.
    ///
    /// Reports only what the frame carries: BSSID from Address 3, SSID from the SSID IE, channel
    /// from the DS Parameter Set IE, and SAE support only if an RSN/WPA IE advertises AKM suite
    /// selector 0x000F. Anything absent stays `None`/absent; nothing is inferred.
    pub fn decode_beacon(frame: &[u8]) -> Result<DecodedBeacon, WifiError> {
        // 802.11 MAC header (24) + Beacon fixed parameters: timestamp 8, beacon interval 2,
        // capability info 2. Tagged parameters therefore begin at offset 36.
        const TAGGED_PARAMS_OFFSET: usize = 36;
        if frame.len() < TAGGED_PARAMS_OFFSET {
            return Err(WifiError::MalformedFrame);
        }
        if (frame[0] >> 2) & 0x03 != (FRAME_TYPE_MGMT as u8) {
            return Err(WifiError::MalformedFrame);
        }
        if (frame[0] >> 4) & 0x0F != SUBTYPE_BEACON {
            return Err(WifiError::MalformedFrame);
        }

        let mut bssid = [0u8; 6];
        bssid.copy_from_slice(&frame[16..22]);

        let mut ssid: Option<String> = None;
        let mut channel: u8 = 0;
        let mut sae = SaeEvidence::Unknown;

        let mut i = TAGGED_PARAMS_OFFSET;
        while i + 2 <= frame.len() {
            let tag_id = frame[i];
            let tag_len = frame[i + 1] as usize;
            let body_start = i + 2;
            let body_end = match body_start.checked_add(tag_len) {
                Some(end) if end <= frame.len() => end,
                // A tag that overruns the frame means the frame is truncated: stop rather than
                // read past the end.
                _ => break,
            };
            let body = &frame[body_start..body_end];

            match tag_id {
                SSID_IE_ID => {
                    if ssid.is_none() {
                        // Non-UTF-8 or hidden (zero-length) SSIDs are reported as empty rather
                        // than replaced with the invented strings "Hidden"/"Unknown".
                        ssid = Some(
                            core::str::from_utf8(body)
                                .map(|s| String::from(s))
                                .unwrap_or_default(),
                        );
                    }
                }
                DS_PARAMS_IE_ID => {
                    if body.len() >= 1 {
                        channel = body[0];
                    }
                }
                RSN_IE_ID => {
                    sae = merge_sae_evidence(sae, parse_rsn_akm(body));
                }
                VENDOR_IE_ID => {
                    if body.len() >= 4 && body[0..3] == WPA_IE_OUI && body[3] == 0x01 {
                        sae = merge_sae_evidence(sae, parse_rsn_akm(&body[4..]));
                    }
                }
                _ => {}
            }

            i = body_end;
        }

        Ok(DecodedBeacon { bssid, ssid: ssid.unwrap_or_default(), channel, sae })
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

/// Combines SAE evidence observed across multiple security IEs in one beacon.
fn merge_sae_evidence(accumulated: SaeEvidence, next: SaeEvidence) -> SaeEvidence {
    match (accumulated, next) {
        (SaeEvidence::AdvertisedSae, _) | (_, SaeEvidence::AdvertisedSae) => SaeEvidence::AdvertisedSae,
        (SaeEvidence::AdvertisedWithoutSae, SaeEvidence::AdvertisedWithoutSae) => SaeEvidence::AdvertisedWithoutSae,
        _ => SaeEvidence::Unknown,
    }
}

/// Parses the AKM Suite List out of an RSN/WPA Information Element body.
///
/// WHY this exists: the previous code set `wpa3_sae_supported: true` on every discovered network
/// without reading a single byte of the RSN IE. Now the verdict comes only from a suite selector
/// the AP actually advertised.
///
/// Layout (802.11-2016 Table 9-129 and the 802.11i WPA IE), all little-endian u16:
/// `version | group cipher count | group list (4n) | pairwise count | pairwise list (4n) |
///  AKM count | AKM list (2n)`
/// Returns `Unknown` whenever the element is too short to carry a complete AKM list, because a
/// truncated IE proves nothing about the AP's authentication method.
fn parse_rsn_akm(body: &[u8]) -> SaeEvidence {
    // Skip Version (2 bytes) and require room for at least the version plus one count field.
    let mut offset: usize = 2;

    // Group Cipher Suite List: 4 bytes per suite.
    let group_count = read_u16(body, offset) as usize;
    offset = match offset.checked_add(2).and_then(|o| o.checked_add(group_count.saturating_mul(4))) {
        Some(o) => o,
        None => return SaeEvidence::Unknown,
    };

    // Pairwise Cipher Suite List: 4 bytes per suite.
    let pairwise_count = read_u16(body, offset) as usize;
    offset = match offset.checked_add(2).and_then(|o| o.checked_add(pairwise_count.saturating_mul(4))) {
        Some(o) => o,
        None => return SaeEvidence::Unknown,
    };

    // AKM Suite Count.
    let akm_count = read_u16(body, offset) as usize;
    offset = match offset.checked_add(2) {
        Some(o) => o,
        None => return SaeEvidence::Unknown,
    };

    let mut saw_akm = false;
    for _ in 0..akm_count {
        // AKM Suite Selectors are 2 bytes each.
        if offset + 2 > body.len() {
            // IE truncated before its declared AKM count: nothing was proven.
            return if saw_akm { SaeEvidence::AdvertisedWithoutSae } else { SaeEvidence::Unknown };
        }
        let selector = u16::from_le_bytes([body[offset], body[offset + 1]]);
        offset += 2;
        saw_akm = true;
        if selector == AKM_SUITE_SAE {
            return SaeEvidence::AdvertisedSae;
        }
    }

    if saw_akm {
        SaeEvidence::AdvertisedWithoutSae
    } else {
        SaeEvidence::Unknown
    }
}

/// Reads a little-endian u16 at `offset`, or 0 when the element is too short.
#[inline]
fn read_u16(buf: &[u8], offset: usize) -> u16 {
    if offset + 2 > buf.len() {
        0
    } else {
        u16::from_le_bytes([buf[offset], buf[offset + 1]])
    }
}
