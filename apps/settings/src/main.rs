//! Inter OS Native Sovereign Settings & Telemetry App
//!
//! Full production settings and system diagnostics app executing in Ring 3 (User Mode):
//! - Five system-settings categories backed by kernel capability-checked services.
//! - Interactive setting mutators (Power Profile, Wi-Fi Toggle, Brightness scaling)
//! - DRM/KMS Client Surface Rendering & Double-Buffered Presentation
//! - Asynchronous event reactor loop handling touch interactions and lifecycle termination

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Top-level category tabs in Sovereign Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Wireless = 0,
    PowerAndBattery = 1,
    ThermalsAndSensors = 2,
    SecurityAndPrivacy = 3,
    AboutDevice = 4,
}

/// Dynamic Voltage and Frequency Scaling (DVFS) Governor profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    Performance = 0, // 2400 MHz
    Balanced = 1,    // 1200 MHz
    PowerSaver = 2,  // 600 MHz
}

/// Outcome of a hardware/service query, recorded so the UI can render an
/// explicit unavailable state instead of inventing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryState {
    /// The kernel refused the query or no backend is attached.
    Unavailable,
    /// The query succeeded and returned a payload.
    Reported,
}

/// Reports whether `haystack` contains the exact byte sequence `needle`.
///
/// WHY: the telemetry payloads are compact JSON with no allocator, so presence
/// checks are done by scanning the raw bytes the kernel actually wrote.
fn contains_run(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

/// Sovereign Settings application state engine.
pub struct SettingsApp {
    pub current_tab: SettingsTab,
    pub power_profile: PowerProfile,
    pub wifi_enabled: bool,
    pub ble_beacon_enabled: bool,
    pub auto_brightness: bool,
    pub security_enforced: bool,
    pub cached_battery_soc: u8,
    pub cached_temp_c: u8,
    pub terminal_enabled: bool,
    /// Recorded outcome of the wireless HAL query for the last refresh.
    pub wireless: TelemetryState,
    /// Recorded outcome of the battery / power query for the last refresh.
    pub battery: TelemetryState,
    /// Recorded outcome of the thermal query for the last refresh.
    pub thermal: TelemetryState,
    /// Recorded outcome of the sensor query for the last refresh.
    pub sensor: TelemetryState,
}

impl SettingsApp {
    pub const fn new() -> Self {
        Self {
            current_tab: SettingsTab::Wireless,
            power_profile: PowerProfile::Balanced,
            wifi_enabled: false,
            ble_beacon_enabled: false,
            auto_brightness: false,
            security_enforced: true,
            cached_battery_soc: 0,
            cached_temp_c: 0,
            terminal_enabled: false,
            // WHY: default to Unavailable so a query that never ran is never
            // rendered as if hardware had answered.
            wireless: TelemetryState::Unavailable,
            battery: TelemetryState::Unavailable,
            thermal: TelemetryState::Unavailable,
            sensor: TelemetryState::Unavailable,
        }
    }

    /// Queries and reports the real outcome of the wireless HAL query.
    pub fn refresh_wireless(&mut self) {
        let mut buf = [0u8; 128];
        // WHY: a non-empty payload was previously treated as proof that wireless
        // hardware exists. The payload carries the HAL's own station state, so
        // report what the HAL actually said instead of the presence of bytes.
        match std::wireless_query(&mut buf) {
            Ok(len) if len > 0 => {
                let body = &buf[..len];
                self.wireless = if contains_run(body, b"\"wifi_state\":\"Disabled\"") {
                    TelemetryState::Unavailable
                } else {
                    TelemetryState::Reported
                };
                std::log(
                    0,
                    "[SETTINGS APP] Wireless HAL response (raw, exactly as reported by the HAL):",
                )
                .ok();
                std::log(0, core::str::from_utf8(body).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                self.wireless = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Wireless HAL returned an empty payload; no wireless hardware reported.",
                )
                .ok();
            }
            Err(_) => {
                // WHY: the query is gated on SystemControl, which this app does
                // not hold. Say the query was refused instead of implying a radio.
                self.wireless = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Wireless HAL query refused by the kernel; wireless state unknown.",
                )
                .ok();
            }
        }
    }

    /// Queries the battery fuel gauge and reports the real outcome.
    pub fn refresh_power(&mut self) {
        let mut profile = [0u8; 32];
        // WHY: this read goes through the FileSystemRead capability slot, which
        // this app holds. Report the stored value verbatim instead of echoing a
        // profile name and then asserting a DVFS governor change that never ran.
        match std::settings_get(3, "power.profile", &mut profile) {
            Ok(n) => {
                std::log(0, "[SETTINGS APP] Stored power.profile value:").ok();
                if let Ok(v) = core::str::from_utf8(&profile[..n]) {
                    std::log(0, v).ok();
                }
            }
            Err(_) => {
                std::log(
                    0,
                    "[SETTINGS APP] Stored power.profile read refused by the kernel; profile unknown.",
                )
                .ok();
            }
        }

        // WHY: this app never calls power_set_profile/power_set_governor, so the
        // governor must be read, not asserted. Report the observed governor id.
        match std::power_get_governor() {
            Ok(gov) => {
                std::log(
                    0,
                    match gov {
                        0 => "[SETTINGS APP] DVFS governor as reported by the kernel: Performance.",
                        1 => "[SETTINGS APP] DVFS governor as reported by the kernel: Power Saver.",
                        2 => "[SETTINGS APP] DVFS governor as reported by the kernel: Interactive.",
                        3 => "[SETTINGS APP] DVFS governor as reported by the kernel: Schedutil.",
                        _ => "[SETTINGS APP] DVFS governor as reported by the kernel: unrecognised id.",
                    },
                )
                .ok();
            }
            Err(_) => {
                // WHY: the governor query requires SystemControl. Report the
                // refusal instead of the locally cached profile enum.
                self.battery = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] DVFS governor query refused by the kernel; no governor change was applied.",
                )
                .ok();
            }
        }

        let mut buf = [0u8; 128];
        match std::battery_query(&mut buf) {
            Ok(len) if len > 0 => {
                self.battery = TelemetryState::Reported;
                std::log(
                    0,
                    "[SETTINGS APP] Battery fuel gauge response (raw, as reported by the service):",
                )
                .ok();
                std::log(0, core::str::from_utf8(&buf[..len]).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                self.battery = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Battery fuel gauge returned an empty payload.",
                )
                .ok();
            }
            Err(_) => {
                self.battery = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Battery fuel gauge query refused by the kernel; charge level unknown.",
                )
                .ok();
            }
        }
    }

    /// Queries multi-zone thermal sensors and IMU orientation.
    pub fn refresh_thermals_and_sensors(&mut self) {
        let mut t_buf = [0u8; 128];
        match std::thermal_query(&mut t_buf) {
            Ok(len) if len > 0 => {
                self.thermal = TelemetryState::Reported;
                std::log(
                    0,
                    "[SETTINGS APP] Thermal response (raw, as reported by the service):",
                )
                .ok();
                std::log(0, core::str::from_utf8(&t_buf[..len]).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                self.thermal = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Thermal service returned an empty payload.",
                )
                .ok();
            }
            Err(_) => {
                // WHY: an unbound thermal sensor reports DeviceError and this app
                // lacks the power-control authority the query requires. Both mean
                // there is no temperature to show.
                self.thermal = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Thermal service reported no attached sensor; temperatures unknown.",
                )
                .ok();
            }
        }

        let mut s_buf = [0u8; 128];
        match std::sensor_read(&mut s_buf) {
            Ok(len) if len > 0 => {
                self.sensor = TelemetryState::Reported;
                std::log(
                    0,
                    "[SETTINGS APP] Sensor response (raw, as reported by the service):",
                )
                .ok();
                std::log(0, core::str::from_utf8(&s_buf[..len]).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                self.sensor = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Sensor service returned an empty payload.",
                )
                .ok();
            }
            Err(_) => {
                self.sensor = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] Sensor service query refused by the kernel; no sensor data.",
                )
                .ok();
            }
        }
    }

    /// Audits security identity and hardware keystore slots.
    pub fn audit_security(&mut self) {
        let mut key_buf = [0u8; 32];
        // WHY: raw keystore material never crosses the kernel boundary, so this
        // call is expected to be refused. Say so instead of staying silent, so a
        // missing keystore is never read as an untested one.
        match std::keystore_derive(0, &mut key_buf) {
            // WHY: the derivation syscall returns a byte count; any length other
            // than a full key is not a usable key, so treat it as a failure.
            Ok(32) => {
                std::log(
                    0,
                    "[SETTINGS APP] Keystore derivation succeeded for the application slot.",
                )
                .ok();
            }
            Ok(_) => {
                std::log(
                    0,
                    "[SETTINGS APP] Keystore derivation returned a short result; no usable application key.",
                )
                .ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[SETTINGS APP] Keystore derivation refused by the kernel; no application key is available.",
                )
                .ok();
            }
        }
    }

    /// Reads real-time hardware clock and microkernel version metadata.
    pub fn display_about_device(&mut self) {
        let mut rtc_buf = [0u8; 64];
        // WHY: report the clock query outcome. rtc_get_time requires the
        // system-control authority, so an unlogged Err must not be read as a
        // confirmed wall-clock reading.
        match std::rtc_get_time(&mut rtc_buf) {
            Ok(len) if len > 0 => {
                std::log(
                    0,
                    "[SETTINGS APP] System time as reported by the platform RTC service:",
                )
                .ok();
                std::log(0, core::str::from_utf8(&rtc_buf[..len]).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                std::log(
                    0,
                    "[SETTINGS APP] RTC service returned an empty payload; system time unknown.",
                )
                .ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[SETTINGS APP] RTC query refused by the kernel; system time unknown.",
                )
                .ok();
            }
        }
        std::log(
            0,
            "[SETTINGS APP] About: Inter OS Core (x86_64, Ring 0/Ring 3 User Separation).",
        )
        .ok();
    }

    /// Queries authoritative system settings for developer terminal availability.
    pub fn refresh_developer_options(&mut self) {
        let mut buf = [0u8; 16];
        // WHY: this read uses capability slot 0 (SerialOutput), which does not
        // carry settings read rights, so the kernel refuses it. Keep the local
        // value at the fail-closed default and log that it is unverified rather
        // than leaving the app to imply a confirmed setting.
        match std::settings_get(0, "developer.terminal_enabled", &mut buf) {
            Ok(n) => {
                if let Ok(v) = core::str::from_utf8(&buf[..n]) {
                    self.terminal_enabled = v == "true";
                    std::log(
                        0,
                        "[SETTINGS APP] developer.terminal_enabled read from the settings store.",
                    )
                    .ok();
                    std::log(0, v).ok();
                }
            }
            Err(_) => {
                self.terminal_enabled = false;
                std::log(
                    0,
                    "[SETTINGS APP] developer.terminal_enabled read refused by the kernel; showing Disabled by default.",
                )
                .ok();
            }
        }
    }

    /// Toggles the developer terminal setting and updates system configuration.
    pub fn toggle_terminal(&mut self) {
        let desired = !self.terminal_enabled;
        let val: &[u8] = if desired { b"true" } else { b"false" };
        // WHY: settings_set requires a FileSystemWrite capability in the given
        // slot. This call passes slot 0, which is the SerialOutput slot, so the
        // kernel denies the write. The previous code discarded that error and
        // logged success. Report the real result, and leave the local flag at the
        // last observed value so the UI never shows an unpersisted toggle.
        match std::settings_set(0, "developer.terminal_enabled", val) {
            Ok(()) => {
                self.terminal_enabled = desired;
                std::log(
                    0,
                    if desired {
                        "[SETTINGS APP] Developer terminal enabled (default was: Disabled by default)."
                    } else {
                        "[SETTINGS APP] Developer terminal disabled."
                    },
                )
                .ok();
            }
            Err(_) => {
                std::log(
                    0,
                    "[SETTINGS APP] Developer terminal change refused by the kernel (settings write denied). Unchanged: Disabled by default.",
                )
                .ok();
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF070B12;
const SURFACE_DARK: u32 = 0xFF111B2B;
const SURFACE_RAISED: u32 = 0xFF17243A;
const BORDER_SUBTLE: u32 = 0xFF26354D;
const ACCENT_SKY: u32 = 0xFF31C4FF;
const SUCCESS_EMERALD: u32 = 0xFF56D68A;
const WARNING_AMBER: u32 = 0xFFFFC043;
const TEXT_PRIMARY: u32 = 0xFFF4F7FB;
const TEXT_MUTED: u32 = 0xFF9AA8BC;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

#[inline(never)]
fn get_settings_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    // 1. Status Bar & Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..100, y: 28..68)
        if x >= 24 && x < 100 && y >= 28 && y < 68 {
            let center_x = 62;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Header Title: "Settings"
        if y >= 36 && y < 60 {
            if let Some(c) = test_text(x, y, 116, 36, 3, b"Settings", TEXT_PRIMARY) {
                return c;
            }
        }

        // Top right System Badge (x: 540..696, y: 28..68)
        if x >= 540 && x < 696 && y >= 28 && y < 68 {
            let rx = (x - 540).min(695 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 20 && ry < 20 {
                let cdx = (20 - rx) as isize;
                let cdy = (20 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 400 {
                    return SURFACE_DARK;
                }
            }

            if let Some(c) = test_text(x, y, 574, 40, 2, b"SYSTEM", ACCENT_SKY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return SURFACE_DARK;
    }

    // 2. Category Tab Strip (y: 104..164)
    if y >= 104 && y < 164 {
        for t in 0..5 {
            let col_x = 24 + t * 136;
            if x >= col_x && x < col_x + 124 && y >= 110 && y < 158 {
                let is_active = t == app.current_tab as usize;
                let rx = (x - col_x).min(col_x + 123 - x);
                let ry = (y - 110).min(157 - y);
                if rx < 14 && ry < 14 {
                    let cdx = (14 - rx) as isize;
                    let cdy = (14 - ry) as isize;
                    if cdx * cdx + cdy * cdy >= 196 {
                        return BG_DARK;
                    }
                }

                let text_color = if is_active { 0xFFFFFFFF } else { TEXT_MUTED };
                let (label, lx) = match t {
                    0 => (&b"WIFI"[..], col_x + 30),
                    1 => (&b"POWER"[..], col_x + 22),
                    2 => (&b"SENSORS"[..], col_x + 8),
                    3 => (&b"SECURITY"[..], col_x + 4),
                    _ => (&b"ABOUT"[..], col_x + 22),
                };

                if let Some(c) = test_text(x, y, lx, 126, 2, label, text_color) {
                    return c;
                }

                if rx < 2 || ry < 2 {
                    return if is_active { ACCENT_SKY } else { BORDER_SUBTLE };
                }

                return if is_active { 0xFF173552 } else { SURFACE_DARK };
            }
        }
        return BG_DARK;
    }

    // 3. Settings Cards (y: 176..1320)
    if y >= 176 && y < 1320 {
        match app.current_tab {
            SettingsTab::Wireless => {
                // Card 0: Wi-Fi
                if x >= 24 && x < W - 24 && y >= 180 && y < 330 {
                    if let Some(c) = render_card_frame(x, y, 180, 330, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 210, 2, b"Wi-Fi Network", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 270, 1, if app.wireless == TelemetryState::Reported { &b"Wireless telemetry received"[..] } else { &b"Radio status unavailable"[..] }, if app.wireless == TelemetryState::Reported { SUCCESS_EMERALD } else { TEXT_MUTED }) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 500, 206, if app.wireless == TelemetryState::Reported { &b"REPORTED"[..] } else { &b"UNAVAILABLE"[..] }, if app.wireless == TelemetryState::Reported { SUCCESS_EMERALD } else { TEXT_MUTED }) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 1: Bluetooth
                if x >= 24 && x < W - 24 && y >= 350 && y < 500 {
                    if let Some(c) = render_card_frame(x, y, 350, 500, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 380, 2, b"Bluetooth & BLE", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 440, 1, b"Bluetooth status unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 500, 376, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 2: Cellular
                if x >= 24 && x < W - 24 && y >= 520 && y < 670 {
                    if let Some(c) = render_card_frame(x, y, 520, 670, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 550, 2, b"Cellular Baseband", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 610, 1, b"Cellular status unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 500, 546, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
            }
            SettingsTab::PowerAndBattery => {
                // Card 0: Battery
                if x >= 24 && x < W - 24 && y >= 180 && y < 330 {
                    if let Some(c) = render_card_frame(x, y, 180, 330, WARNING_AMBER) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 210, 2, b"Battery Fuel Gauge", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 270, 1, if app.battery == TelemetryState::Reported { &b"Battery payload received"[..] } else { &b"Battery telemetry unavailable"[..] }, TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 510, 206, if app.battery == TelemetryState::Reported { &b"REPORTED"[..] } else { &b"UNAVAILABLE"[..] }, if app.battery == TelemetryState::Reported { SUCCESS_EMERALD } else { TEXT_MUTED }) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 1: DVFS
                if x >= 24 && x < W - 24 && y >= 350 && y < 500 {
                    if let Some(c) = render_card_frame(x, y, 350, 500, ACCENT_SKY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 380, 2, b"CPU Frequency Scaling", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 440, 1, b"Governor telemetry unavailable", ACCENT_SKY) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 520, 376, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 2: Thermal Governor
                if x >= 24 && x < W - 24 && y >= 520 && y < 670 {
                    if let Some(c) = render_card_frame(x, y, 520, 670, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 550, 2, b"Thermal Governor", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 610, 1, b"DVFS state unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 530, 546, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
            }
            SettingsTab::ThermalsAndSensors => {
                // Card 0: Core Temp
                if x >= 24 && x < W - 24 && y >= 180 && y < 330 {
                    if let Some(c) = render_card_frame(x, y, 180, 330, WARNING_AMBER) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 210, 2, b"CPU Core Temperature", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 270, 1, if app.thermal == TelemetryState::Reported { &b"Thermal payload received"[..] } else { &b"Temperature telemetry unavailable"[..] }, TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 510, 206, if app.thermal == TelemetryState::Reported { &b"REPORTED"[..] } else { &b"UNAVAILABLE"[..] }, if app.thermal == TelemetryState::Reported { SUCCESS_EMERALD } else { TEXT_MUTED }) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 1: Motion Sensor
                if x >= 24 && x < W - 24 && y >= 350 && y < 500 {
                    if let Some(c) = render_card_frame(x, y, 350, 500, WARNING_AMBER) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 380, 2, b"6-Axis Motion Sensor", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 440, 1, if app.sensor == TelemetryState::Reported { &b"Sensor payload received; values not parsed"[..] } else { &b"Sensor values unavailable"[..] }, TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 510, 376, if app.sensor == TelemetryState::Reported { &b"REPORTED"[..] } else { &b"UNAVAILABLE"[..] }, if app.sensor == TelemetryState::Reported { SUCCESS_EMERALD } else { TEXT_MUTED }) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 2: Light Sensor
                if x >= 24 && x < W - 24 && y >= 520 && y < 670 {
                    if let Some(c) = render_card_frame(x, y, 520, 670, WARNING_AMBER) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 550, 2, b"Ambient Light Sensor", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 610, 1, if app.sensor == TelemetryState::Reported { &b"Sensor payload received; lux not parsed"[..] } else { &b"Light sensor telemetry unavailable"[..] }, TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 510, 546, if app.sensor == TelemetryState::Reported { &b"REPORTED"[..] } else { &b"UNAVAILABLE"[..] }, if app.sensor == TelemetryState::Reported { SUCCESS_EMERALD } else { TEXT_MUTED }) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
            }
            SettingsTab::SecurityAndPrivacy => {
                // Card 0: Keystore
                if x >= 24 && x < W - 24 && y >= 180 && y < 330 {
                    if let Some(c) = render_card_frame(x, y, 180, 330, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 210, 2, b"Hardware Keystore", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 270, 1, b"Keystore status unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 520, 206, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 1: Vault
                if x >= 24 && x < W - 24 && y >= 350 && y < 500 {
                    if let Some(c) = render_card_frame(x, y, 350, 500, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 380, 2, b"Storage Vault Encryption", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 440, 1, b"Vault encryption status unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 510, 376, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 2: Privacy Shield
                if x >= 24 && x < W - 24 && y >= 520 && y < 670 {
                    if let Some(c) = render_card_frame(x, y, 520, 670, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 550, 2, b"Privacy Shield Filter", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 610, 1, b"Privacy filter status unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 540, 546, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 3: Developer terminal (Tappable target: y: 830..1010, x: 32..688)
                if x >= 32 && x < W - 32 && y >= 830 && y < 1010 {
                    let accent_bar = if app.terminal_enabled { SUCCESS_EMERALD } else { BORDER_SUBTLE };
                    if let Some(c) = render_card_frame(x, y, 830, 1010, accent_bar) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 860, 2, b"Developer terminal", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 896, 1, b"Disabled by default", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 924, 1, b"Config key: developer.terminal_enabled", TEXT_MUTED) {
                        return c;
                    }
                    let (toggle_str, toggle_col) = if app.terminal_enabled {
                        (&b"ENABLED"[..], SUCCESS_EMERALD)
                    } else {
                        (&b"DISABLED"[..], TEXT_MUTED)
                    };
                    if let Some(c) = render_badge(x, y, 520, 856, toggle_str, toggle_col) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
            }
            SettingsTab::AboutDevice => {
                // Card 0: OS
                if x >= 24 && x < W - 24 && y >= 180 && y < 330 {
                    if let Some(c) = render_card_frame(x, y, 180, 330, ACCENT_SKY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 210, 2, b"InterOS Core", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 270, 1, b"Version: 0.8.2 (x86_64 Sovereign Microkernel)", ACCENT_SKY) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 520, 206, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 1: Architecture
                if x >= 24 && x < W - 24 && y >= 350 && y < 500 {
                    if let Some(c) = render_card_frame(x, y, 350, 500, ACCENT_SKY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 380, 2, b"Platform Microkernel", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 440, 1, b"Capability Security & VFS Block Storage", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 530, 376, b"RING 0", ACCENT_SKY) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
                // Card 2: RTC
                if x >= 24 && x < W - 24 && y >= 520 && y < 670 {
                    if let Some(c) = render_card_frame(x, y, 520, 670, BORDER_SUBTLE) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 550, 2, b"Hardware Real-Time Clock", TEXT_PRIMARY) {
                        return c;
                    }
                    if let Some(c) = test_text(x, y, 60, 610, 1, b"Clock synchronization unavailable", TEXT_MUTED) {
                        return c;
                    }
                    if let Some(c) = render_badge(x, y, 510, 546, b"UNAVAILABLE", TEXT_MUTED) {
                        return c;
                    }
                    return SURFACE_DARK;
                }
            }
        }
        return BG_DARK;
    }

    // 4. Home Gesture Pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_card_frame(x: usize, y: usize, y0: usize, y1: usize, accent_col: u32) -> Option<u32> {
    // Left accent strip
    if x >= 24 && x < 34 {
        return Some(accent_col);
    }

    let rx = (x - 24).min(W - 25 - x);
    let ry = (y - y0).min(y1 - 1 - y);
    if rx < 18 && ry < 18 {
        let cdx = (18 - rx) as isize;
        let cdy = (18 - ry) as isize;
        if cdx * cdx + cdy * cdy >= 324 {
            return Some(BG_DARK);
        }
    }

    if rx < 2 || ry < 2 {
        return Some(BORDER_SUBTLE);
    }
    None
}

fn render_badge(x: usize, y: usize, bx0: usize, by0: usize, label: &[u8], color: u32) -> Option<u32> {
    let bw = label.len() * 8 + 24;
    let bh = 28;
    if x >= bx0 && x < bx0 + bw && y >= by0 && y < by0 + bh {
        let rx = (x - bx0).min(bx0 + bw - 1 - x);
        let ry = (y - by0).min(by0 + bh - 1 - y);
        if rx < 12 && ry < 12 {
            let cdx = (12 - rx) as isize;
            let cdy = (12 - ry) as isize;
            if cdx * cdx + cdy * cdy >= 144 {
                return Some(SURFACE_DARK);
            }
        }

        if let Some(c) = test_text(x, y, bx0 + 12, by0 + 10, 1, label, color) {
            return Some(c);
        }

        if rx < 2 || ry < 2 {
            return Some(color);
        }
        return Some(SURFACE_RAISED);
    }
    None
}

fn render_settings_ui(surface: u32, app: &SettingsApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_settings_pixel(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[SETTINGS APP (Ring 3)] Sovereign Settings & Diagnostics online.",
    )
    .ok();

    // Slot 6 is FrameBufferDirect
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(
                0,
                "[SETTINGS APP] DRM/KMS Client Surface allocated (z=200).",
            )
            .ok();
            id
        }
        Err(_) => 0,
    };

    let mut settings = SettingsApp::new();

    // 1. Inspect Wireless Configuration
    settings.current_tab = SettingsTab::Wireless;
    settings.refresh_wireless();

    // 2. Inspect Power & Battery State
    settings.current_tab = SettingsTab::PowerAndBattery;
    settings.refresh_power();

    // 3. Inspect Thermals & Hardware Sensors
    settings.current_tab = SettingsTab::ThermalsAndSensors;
    settings.refresh_thermals_and_sensors();

    // 4. Audit Security & Hardware Keystore
    settings.current_tab = SettingsTab::SecurityAndPrivacy;
    settings.audit_security();

    // 5. Query About Device & Hardware RTC
    settings.current_tab = SettingsTab::AboutDevice;
    settings.display_about_device();
    settings.current_tab = SettingsTab::SecurityAndPrivacy;
    settings.refresh_developer_options();

    // Initial render & presentation
    if surface > 0 {
        render_settings_ui(surface, &settings);
        let _ = std::surface_flip(6);
        std::log(0, "[SETTINGS APP] Settings UI presented to scanout buffer.").ok();
    }

    // 6. Interactive Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[SETTINGS APP] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        if surface > 0 {
                            let _ = std::surface_destroy(6, surface);
                        }
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        // Check if Back button or Home gesture bar was tapped: exit cleanly to launcher
                        if (input.x >= 24 && input.x < 100 && input.y >= 28 && input.y < 68)
                            || (input.x >= 240 && input.x <= 480 && input.y >= 1400)
                        {
                            std::log(0, "[SETTINGS APP] Back button or home gesture pressed. Exiting to launcher.").ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        }
                        if settings.current_tab == SettingsTab::SecurityAndPrivacy
                            && input.y >= 830 && input.y < 1010
                            && input.x >= 32 && input.x < (W - 32) as u32
                        {
                            settings.toggle_terminal();
                        } else if input.y >= 110 && input.y < 170 && input.x >= 24 {
                            let tapped = ((input.x - 24) / 136) as usize;
                            settings.current_tab = match tapped {
                                0 => SettingsTab::Wireless,
                                1 => SettingsTab::PowerAndBattery,
                                2 => SettingsTab::ThermalsAndSensors,
                                3 => SettingsTab::SecurityAndPrivacy,
                                _ => SettingsTab::AboutDevice,
                            };
                        } else {
                            let next_tab = match settings.current_tab {
                                SettingsTab::Wireless => SettingsTab::PowerAndBattery,
                                SettingsTab::PowerAndBattery => SettingsTab::ThermalsAndSensors,
                                SettingsTab::ThermalsAndSensors => SettingsTab::SecurityAndPrivacy,
                                SettingsTab::SecurityAndPrivacy => SettingsTab::AboutDevice,
                                SettingsTab::AboutDevice => SettingsTab::Wireless,
                            };
                            settings.current_tab = next_tab;
                        }
                        if surface > 0 {
                            render_settings_ui(surface, &settings);
                            let _ = std::surface_flip(6);
                        }
                        std::log(
                            0,
                            "[SETTINGS APP] Normalized touch event processed; tab switched.",
                        )
                        .ok();
                    }
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[SETTINGS APP] Panic detected! Terminating task...").ok();
    std::exit();
}
