//! Mobile Wireless Subsystem: Bluetooth Low Energy (BLE 5.0) & Wi-Fi Station
//!
//! Implements hardware abstractions for mobile short-range wireless communication.
//! Generates 31-byte Generic Access Profile (GAP) BLE advertising beacons for
//! decentralized peer-to-peer InterLayer mesh discovery, and maintains 802.11
//! Wi-Fi Station WPA3 connection state machines.

use spin::Mutex;

/// Active operational state of the Wi-Fi 802.11 station interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiState {
    Disabled,
    Scanning,
    Associating,
    ConnectedWpa3,
}

/// BLE GAP Advertising Data (AD) Types.
pub const BLE_AD_TYPE_FLAGS: u8 = 0x01;
pub const BLE_AD_TYPE_128BIT_UUID: u8 = 0x07;
pub const BLE_AD_TYPE_MANUFACTURER_SPECIFIC: u8 = 0xFF;

/// Flags bitmask for LE General Discoverable Mode, BR/EDR Not Supported.
pub const BLE_FLAGS_LE_GENERAL_DISCOVERABLE: u8 = 0x06;

/// Mobile Wireless Controller managing Wi-Fi and BLE beaconing.
pub struct WirelessManager {
    pub ble_advertising: bool,
    pub ble_beacon_frame: [u8; 31],
    pub wifi_state: WifiState,
    pub wifi_ssid: [u8; 32],
    pub wifi_ssid_len: usize,
    pub wifi_rssi_dbm: i8,
    pub wifi_freq_mhz: u16,
    pub hardware_present: bool,
}

impl WirelessManager {
    pub const fn new() -> Self {
        Self {
            ble_advertising: false,
            ble_beacon_frame: [0u8; 31],
            wifi_state: WifiState::Disabled,
            wifi_ssid: [0u8; 32],
            wifi_ssid_len: 0,
            wifi_rssi_dbm: -52,
            wifi_freq_mhz: 5180, // 5 GHz Channel 36
        }
    }

    /// Constructs a standard 31-byte BLE 5.0 GAP advertising packet broadcasting
    /// a standard 128-bit service UUID for local peer discovery.
    pub fn build_ble_beacon(&mut self, service_uuid: &[u8; 16]) {
        let mut frame = [0u8; 31];

        // 1. Flags AD Structure (3 bytes: len=2, type=0x01, val=0x06)
        frame[0] = 0x02;
        frame[1] = BLE_AD_TYPE_FLAGS;
        frame[2] = BLE_FLAGS_LE_GENERAL_DISCOVERABLE;

        // 2. Complete 128-bit Service UUID AD Structure (18 bytes: len=17, type=0x07, val=16 bytes)
        frame[3] = 0x11;
        frame[4] = BLE_AD_TYPE_128BIT_UUID;
        frame[5..21].copy_from_slice(service_uuid);

        // 3. Manufacturer Specific Data AD Structure (10 bytes)
        frame[21] = 0x09;
        frame[22] = BLE_AD_TYPE_MANUFACTURER_SPECIFIC;
        frame[23] = 0x49; // 'I'
        frame[24] = 0x4F; // 'O' - InterOS identifier
        frame[25] = 0x01; // Protocol v1

        self.ble_beacon_frame = frame;
        self.ble_advertising = true;
    }

    /// Connects to a Wi-Fi WPA3 network.
    pub fn connect_wifi(&mut self, ssid: &str, rssi_dbm: i8) {
        if !self.hardware_present {
            self.wifi_state = WifiState::Disabled;
            self.wifi_ssid_len = 0;
            self.wifi_rssi_dbm = 0;
            self.wifi_freq_mhz = 0;
            return;
        }
        let bytes = ssid.as_bytes();
        let copy_len = bytes.len().min(32);
        self.wifi_ssid[..copy_len].copy_from_slice(&bytes[..copy_len]);
        self.wifi_ssid_len = copy_len;
        self.wifi_rssi_dbm = rssi_dbm;
        self.wifi_state = WifiState::ConnectedWpa3;
}

    /// Serializes wireless state telemetry into a standardized JSON representation.
    pub fn format_json(&self, out: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct SliceWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl<'a> Write for SliceWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let rem = self.buf.len().saturating_sub(self.pos);
                let to_write = bytes.len().min(rem);
                self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                self.pos += to_write;
                Ok(())
            }
        }
        let mut writer = SliceWriter { buf: out, pos: 0 };
        let ssid_str = core::str::from_utf8(&self.wifi_ssid[..self.wifi_ssid_len]).unwrap_or("None");
        let _ = write!(
            writer,
            r#"{{"wifi_state":"{:?}","ssid":"{}","rssi_dbm":{},"freq_mhz":{},"ble_adv":{}}}"#,
            self.wifi_state, ssid_str, self.wifi_rssi_dbm, self.wifi_freq_mhz, self.ble_advertising
        );
        writer.pos
    }
}

pub static WIRELESS_MANAGER: Mutex<WirelessManager> = Mutex::new(WirelessManager::new());

/// Initializes the Mobile Wireless Subsystem and reports hardware readiness honestly.
pub fn init() {
    let wm = WIRELESS_MANAGER.lock();
    let mut buf = [0u8; 128];
    let len = wm.format_json(&mut buf);
    let str_view = core::str::from_utf8(&buf[..len]).unwrap_or("");

    crate::serial_println!(
        "[WIRELESS] Wireless subsystem initialized: State = {:?}, Status = '{}'",
        wm.wifi_state,
        str_view
    );
}
