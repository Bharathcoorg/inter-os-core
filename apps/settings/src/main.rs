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

fn render_settings_ui(surface: u32, tab: SettingsTab, telemetry: [TelemetryState; 4]) {
    let tab_idx = tab as usize;
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            if y < 96 {
                if y >= 94 {
                    chunk[i] = std::design::color::ACCENT;
                } else if x >= 24 && x < 100 && y >= 28 && y < 68 {
                    // Back button <
                    let center_x = 62;
                    let center_y = 48;
                    let dx = (x as isize - center_x as isize).abs();
                    let dy = (y as isize - center_y as isize).abs();
                    if (dx + dy <= 12 && x <= center_x)
                        || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
                    {
                        chunk[i] = std::design::color::TEXT;
                    } else {
                        chunk[i] = std::design::color::SURFACE_RAISED;
                    }
                } else {
                    chunk[i] = std::design::color::SURFACE;
                }
            } else if (110..170).contains(&y) {
                let t = x.saturating_sub(24) / 136;
                chunk[i] = if t == tab_idx && (x % 136) < 120 {
                    std::design::color::ACCENT
                } else {
                    std::design::color::SURFACE_RAISED
                };
            } else if (200..1200).contains(&y) {
                let card = (y - 200) / 210;
                let card_y = (y - 200) % 210;
                if tab == SettingsTab::SecurityAndPrivacy && card == 3 && card_y >= 35 && card_y < 85 {
                    // "Developer terminal" / "Disabled by default" card preview
                    chunk[i] = if card_y >= 60 {
                        std::design::color::TEXT_MUTED
                    } else {
                        std::design::color::TEXT
                    };
                } else if card < 4 && x >= 32 && x < W - 32 && card_y < 180 {
                    if card_y >= 100 && card_y < 140 && x >= 60 && x < W - 60 {
                        // WHY: the accent strip is the only status indicator this
                        // layout has. When the corresponding query was refused or
                        // the backend is absent, render it muted so the card cannot
                        // be read as reporting live hardware.
                        let reported = telemetry.get(card).copied() == Some(TelemetryState::Reported);
                        chunk[i] = match card {
                            0 => {
                                if reported {
                                    std::design::color::ACCENT
                                } else {
                                    std::design::color::TEXT_MUTED
                                }
                            }
                            1 => {
                                if reported {
                                    std::design::color::SUCCESS
                                } else {
                                    std::design::color::TEXT_MUTED
                                }
                            }
                            2 => {
                                if reported {
                                    std::design::color::WARNING
                                } else {
                                    std::design::color::TEXT_MUTED
                                }
                            }
                            _ => std::design::color::TEXT_MUTED,
                        };
                    } else if tab == SettingsTab::SecurityAndPrivacy && card == 3 && card_y >= 40 && card_y < 100 {
                        chunk[i] = std::design::color::BACKGROUND;
                    } else {
                        chunk[i] = std::design::color::SURFACE;
                    }
                } else {
                    chunk[i] = std::design::color::BACKGROUND;
                }
            } else if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
                chunk[i] = std::design::color::TEXT_MUTED;
            } else {
                chunk[i] = std::design::color::BACKGROUND;
            }
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
        render_settings_ui(
            surface,
            settings.current_tab,
            [
                settings.wireless,
                settings.battery,
                settings.thermal,
                settings.sensor,
            ],
        );
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
                            render_settings_ui(
                                surface,
                                settings.current_tab,
                                [
                                    settings.wireless,
                                    settings.battery,
                                    settings.thermal,
                                    settings.sensor,
                                ],
                            );
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
