// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Mobile Wireless Subsystem: Bluetooth Low Energy (BLE 5.0) & Wi-Fi Station
//!
//! PLATFORM TRUTH:
//! No wireless silicon is bound on this target platform. The `hardware_present`
//! flag is a static constant (`false`), not a hardware detection probe. Because
//! no physical radio hardware exists, no link rates, RSSI, or SSIDs are measured
//! or fabricated. All wireless operations fail closed. State machines generate
//! 31-byte GAP BLE advertising frames and maintain station state models for
//! future hardware bindings.

use spin::Mutex;

/// Active operational state of the Wi-Fi 802.11 station interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiState {
    Disabled,
    Scanning,
    Associating,
    ConnectedWpa3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothState { Disabled, Scanning, Pairing, Connected }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairedDevice {
    pub address: [u8; 6],
    pub profile_mask: u8,
}

/// BLE GAP Advertising Data (AD) Types.
pub const BLE_AD_TYPE_FLAGS: u8 = 0x01;
pub const BLE_AD_TYPE_128BIT_UUID: u8 = 0x07;
pub const BLE_AD_TYPE_MANUFACTURER_SPECIFIC: u8 = 0xFF;

/// Flags bitmask for LE General Discoverable Mode, BR/EDR Not Supported.
pub const BLE_FLAGS_LE_GENERAL_DISCOVERABLE: u8 = 0x06;

/// Mobile Wireless Subsystem state manager.
///
/// PLATFORM TRUTH:
/// No wireless silicon (Wi-Fi PHY/MAC, Bluetooth HCI controller) is bound on this
/// platform. The `hardware_present` flag is a static constant (`false`) set at
/// construction; it is NOT the result of dynamic hardware probing or silicon
/// detection. Because no physical radio is bound, no real RSSI, SSID, frequency,
/// or link rates can be measured or reported. All wireless operations fail closed.
pub struct WirelessManager {
    pub ble_advertising: bool,
    pub ble_beacon_frame: [u8; 31],
    pub wifi_state: WifiState,
    pub wifi_ssid: [u8; 32],
    pub wifi_ssid_len: usize,
    pub wifi_rssi_dbm: i8,
    pub wifi_freq_mhz: u16,
    /// Static flag indicating whether physical wireless silicon is bound.
    /// Permanently `false` on this platform (static constant, not a dynamic probe).
    pub hardware_present: bool,
    pub bluetooth_state: BluetoothState,
    pub paired_devices: [Option<PairedDevice>; 8],
}

impl WirelessManager {
    pub const fn new() -> Self {
        Self {
            ble_advertising: false,
            ble_beacon_frame: [0u8; 31],
            wifi_state: WifiState::Disabled,
            wifi_ssid: [0u8; 32],
            wifi_ssid_len: 0,
            wifi_rssi_dbm: 0,
            wifi_freq_mhz: 0,
            hardware_present: false,
            bluetooth_state: BluetoothState::Disabled,
            paired_devices: [None; 8]
        }
    }

    /// Constructs a standard 31-byte BLE 5.0 GAP advertising packet broadcasting
    /// a standard 128-bit service UUID for local peer discovery.
    pub fn build_ble_beacon(&mut self, service_uuid: &[u8; 16]) {
        if !self.hardware_present { self.ble_advertising = false; return; }
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
    pub fn connect_wifi(&mut self, ssid: &str, rssi_dbm: i8) -> Result<(), &'static str> {
        if !self.hardware_present {
            self.wifi_state = WifiState::Disabled;
            self.wifi_ssid_len = 0;
            self.wifi_rssi_dbm = 0;
            self.wifi_freq_mhz = 0;
            return Err("wifi hardware unavailable");
        }
        let bytes = ssid.as_bytes();
        let copy_len = bytes.len().min(32);
        self.wifi_ssid[..copy_len].copy_from_slice(&bytes[..copy_len]);
        self.wifi_ssid_len = copy_len;
        self.wifi_rssi_dbm = rssi_dbm;
        self.wifi_state = WifiState::ConnectedWpa3;
        Ok(())
    }

    pub fn set_bluetooth_available(&mut self, available: bool) {
        if !self.hardware_present {
            self.bluetooth_state = BluetoothState::Disabled;
            return;
        }
        self.bluetooth_state = if available { BluetoothState::Scanning } else { BluetoothState::Disabled };
    }

    pub fn pair_bluetooth(&mut self, address: [u8; 6], profile_mask: u8) -> Result<(), &'static str> {
        if !self.hardware_present || self.bluetooth_state == BluetoothState::Disabled {
            return Err("bluetooth hardware unavailable");
        }
        if let Some(slot) = self.paired_devices.iter_mut().find(|d| d.map(|v| v.address) == Some(address)) {
            *slot = Some(PairedDevice { address, profile_mask });
            self.bluetooth_state = BluetoothState::Connected;
            return Ok(());
        }
        let slot = self.paired_devices.iter_mut().find(|d| d.is_none()).ok_or("paired device limit reached")?;
        *slot = Some(PairedDevice { address, profile_mask });
        self.bluetooth_state = BluetoothState::Connected;
        Ok(())
    }

    /// Serializes wireless state telemetry into a standardized JSON representation.
    ///
    /// When no wireless silicon is bound (`hardware_present: false`), states plainly
    /// that no silicon is bound and that the flag is static. Does not fabricate
    /// RSSI, SSID, or frequency telemetry.
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
        if !self.hardware_present {
            let _ = write!(
                writer,
                r#"{{"hardware_present":false,"silicon_bound":false,"flag":"static","wifi_state":"{:?}","ble_adv":{}}}"#,
                self.wifi_state, self.ble_advertising
            );
        } else {
            let ssid_str = core::str::from_utf8(&self.wifi_ssid[..self.wifi_ssid_len]).unwrap_or("None");
            let _ = write!(
                writer,
                r#"{{"hardware_present":true,"silicon_bound":true,"flag":"bound","wifi_state":"{:?}","ssid":"{}","rssi_dbm":{},"freq_mhz":{},"ble_adv":{}}}"#,
                self.wifi_state, ssid_str, self.wifi_rssi_dbm, self.wifi_freq_mhz, self.ble_advertising
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_hardware_fails_closed_for_all_wireless_operations() {
        let mut wm = WirelessManager::new();
        assert!(!wm.hardware_present);
        assert_eq!(wm.wifi_state, WifiState::Disabled);
        assert_eq!(wm.bluetooth_state, BluetoothState::Disabled);

        // Wi-Fi connect must fail when hardware is absent
        assert_eq!(wm.connect_wifi("TestSSID", -60), Err("wifi hardware unavailable"));
        assert_eq!(wm.wifi_state, WifiState::Disabled);

        // Bluetooth scanning/enabling must fail when hardware is absent
        wm.set_bluetooth_available(true);
        assert_eq!(wm.bluetooth_state, BluetoothState::Disabled, "Bluetooth cannot be enabled without hardware");

        // Bluetooth pairing must fail when hardware is absent
        let res = wm.pair_bluetooth([0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC], 0x01);
        assert_eq!(res, Err("bluetooth hardware unavailable"));
        assert_eq!(wm.bluetooth_state, BluetoothState::Disabled);

        // BLE beaconing must fail when hardware is absent
        wm.build_ble_beacon(&[0x42; 16]);
        assert!(!wm.ble_advertising);

        // format_json must truthfully reflect absent hardware and disabled wifi,
        // stating plainly that no silicon is bound and the flag is static.
        // It must not fabricate RSSI, SSID, or frequency telemetry.
        let mut buf = [0u8; 128];
        let len = wm.format_json(&mut buf);
        let s = core::str::from_utf8(&buf[..len]).unwrap();
        assert!(s.contains("\"wifi_state\":\"Disabled\""));
        assert!(s.contains("\"hardware_present\":false"));
        assert!(s.contains("\"silicon_bound\":false"), "status JSON must state no silicon is bound");
        assert!(s.contains("\"flag\":\"static\""), "status JSON must state flag is static");
        assert!(!s.contains("\"rssi_dbm\""), "must not fabricate RSSI telemetry when hardware is absent");
        assert!(!s.contains("\"freq_mhz\""), "must not fabricate frequency telemetry when hardware is absent");
        assert!(!s.contains("\"ssid\""), "must not fabricate SSID telemetry when hardware is absent");
    }
}
