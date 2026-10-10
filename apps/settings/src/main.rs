//! Inter OS Core "System Space" — Sovereign Settings & Telemetry App
//!
//! Ring 3 (User Mode) presentation of the design system's screen 6,
//! "System Space" (`Ui Samples/INTEROS_UI_DESIGN_SYSTEM.md`):
//! - A LIGHT semantic category surface: a 2-column grid of the seven canonical
//!   categories (Device, Network, Personal, Apps, Power, Security, System).
//! - Stack navigation: tapping a category pushes a detail screen with a
//!   48x48 back target that pops back to the landing grid.
//! - Every hardware/service line is backed by a capability-checked kernel
//!   query recorded in `SettingsApp`; anything the kernel does not answer is
//!   rendered with the design system's honest unavailable treatment and never
//!   with a fabricated value.
//! - Typography: every text run is measured against the font's advance table
//!   (`interos_std::design::font::measure*` / `advance_of`) and fitted to its
//!   box with an explicit ellipsis pass before a pixel is drawn, so no label
//!   is ever sliced mid-glyph or allowed to collide with a badge or a card
//!   edge.
//! - DRM/KMS Client Surface Rendering & Double-Buffered Presentation
//! - Asynchronous event reactor loop handling touch interactions and lifecycle termination

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// System Space categories (design system screen 6)
// -----------------------------------------------------------------------------

/// The seven semantic System Space categories, in landing-grid order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Device = 0,
    Network = 1,
    Personal = 2,
    Apps = 3,
    Power = 4,
    Security = 5,
    System = 6,
}

impl Section {
    pub const COUNT: usize = 7;

    pub fn from_index(i: usize) -> Option<Section> {
        match i {
            0 => Some(Section::Device),
            1 => Some(Section::Network),
            2 => Some(Section::Personal),
            3 => Some(Section::Apps),
            4 => Some(Section::Power),
            5 => Some(Section::Security),
            6 => Some(Section::System),
            _ => None,
        }
    }

    /// Detail-screen header title. Static data; no allocation.
    pub fn title(self) -> &'static [u8] {
        match self {
            Section::Device => b"Device",
            Section::Network => b"Network",
            Section::Personal => b"Personal",
            Section::Apps => b"Apps",
            Section::Power => b"Power",
            Section::Security => b"Security",
            Section::System => b"System",
        }
    }
}

/// Dynamic Voltage and Frequency Scaling (DVFS) Governor profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    Performance = 0,
    Balanced = 1,
    PowerSaver = 2,
}

impl PowerProfile {
    /// Index of the profile in the on-screen selector row.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Wire id for `power_set_profile` (kernel: 0=PowerSave, 1=Balanced,
    /// 2=Performance, 3=Turbo).
    fn syscall_id(self) -> u32 {
        match self {
            PowerProfile::PowerSaver => 0,
            PowerProfile::Balanced => 1,
            PowerProfile::Performance => 2,
        }
    }
}

/// Honest record of the most recent DVFS profile request. The kernel gates
/// `power_set_profile` on SystemControl, so a request can be refused; the UI
/// must report the real outcome instead of assuming the change landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFeedback {
    /// No request has been made yet in this session.
    Unattempted,
    /// The kernel accepted the profile and reported the resulting ceiling (MHz).
    Applied(u32),
    /// The kernel refused the request; no governor state was changed.
    Refused,
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
    /// `None` = landing grid; `Some(section)` = that category's detail screen.
    pub current_section: Option<Section>,
    /// Last profile the kernel accepted (initial value is the neutral default
    /// of a freshly booted system, applied here for control highlight only).
    pub power_profile: PowerProfile,
    /// Honest outcome of the most recent DVFS profile request.
    pub profile_feedback: ProfileFeedback,
    /// Last DVFS governor id the kernel reported, when the query answered.
    pub governor: Option<u32>,
    pub terminal_enabled: bool,
    /// Recorded outcome of the RTC query for the About card.
    pub rtc: TelemetryState,
    /// Raw RTC payload exactly as the platform reported it.
    /// Only meaningful when `rtc == TelemetryState::Reported`.
    pub rtc_time: [u8; 64],
    pub rtc_time_len: usize,
    /// `about.os_version` read from the settings store; length 0 when the
    /// store did not answer, so the card never shows a compile-time literal.
    pub os_version: [u8; 24],
    pub os_version_len: usize,
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
            current_section: None,
            power_profile: PowerProfile::Balanced,
            profile_feedback: ProfileFeedback::Unattempted,
            governor: None,
            terminal_enabled: false,
            // WHY: Unavailable/zero until the About screen's queries answer;
            // a card must never render a value that was never read.
            rtc: TelemetryState::Unavailable,
            rtc_time: [0u8; 64],
            rtc_time_len: 0,
            os_version: [0u8; 24],
            os_version_len: 0,
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

    /// Queries the battery fuel gauge and the DVFS governor and reports the
    /// real outcome of each.
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

        // WHY: the observed governor must be read, not asserted. Record the id
        // the kernel actually reported so the Power detail can render it.
        match std::power_get_governor() {
            Ok(gov) => {
                self.governor = Some(gov);
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
                self.governor = None;
                std::log(
                    0,
                    "[SETTINGS APP] DVFS governor query refused by the kernel; governor state unknown.",
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

    /// Requests a DVFS profile from the kernel and records the real outcome.
    ///
    /// WHY the result is recorded rather than assumed: `power_set_profile` is
    /// gated on the SystemControl authority this app does not hold, so the
    /// kernel can refuse. The local selection only moves on `Ok`, and the
    /// reason is rendered verbatim so the control can never claim a change
    /// that did not happen.
    pub fn apply_power_profile(&mut self, profile: PowerProfile) {
        match std::power_set_profile(profile.syscall_id()) {
            Ok(mhz) => {
                self.power_profile = profile;
                self.profile_feedback = ProfileFeedback::Applied(mhz);
                std::log(
                    0,
                    "[SETTINGS APP] Kernel accepted the requested DVFS profile; frequency ceiling reported.",
                )
                .ok();
            }
            Err(_) => {
                self.profile_feedback = ProfileFeedback::Refused;
                std::log(
                    0,
                    "[SETTINGS APP] DVFS profile change refused by the kernel; no governor state was changed.",
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
                // Record the reading so the System detail renders the payload
                // the platform actually returned instead of a fixed
                // "unavailable" line that ignored this successful query.
                let n = len.min(self.rtc_time.len());
                self.rtc_time[..n].copy_from_slice(&rtc_buf[..n]);
                self.rtc_time_len = n;
                self.rtc = TelemetryState::Reported;
                std::log(
                    0,
                    "[SETTINGS APP] System time as reported by the platform RTC service:",
                )
                .ok();
                std::log(0, core::str::from_utf8(&rtc_buf[..len]).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                self.rtc_time_len = 0;
                self.rtc = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] RTC service returned an empty payload; system time unknown.",
                )
                .ok();
            }
            Err(_) => {
                self.rtc_time_len = 0;
                self.rtc = TelemetryState::Unavailable;
                std::log(
                    0,
                    "[SETTINGS APP] RTC query refused by the kernel; system time unknown.",
                )
                .ok();
            }
        }
        // WHY: the About card used to print a compile-time "0.8.2" literal.
        // Read the authoritative value from the settings store through
        // capability slot 3 (the FileSystemRead slot that reads power.profile);
        // when the store does not answer, the card reports that instead.
        let mut ver_buf = [0u8; 24];
        match std::settings_get(3, "about.os_version", &mut ver_buf) {
            Ok(n) if n > 0 => {
                let n = n.min(self.os_version.len());
                self.os_version[..n].copy_from_slice(&ver_buf[..n]);
                self.os_version_len = n;
                std::log(0, "[SETTINGS APP] about.os_version read from the settings store.").ok();
                std::log(0, core::str::from_utf8(&self.os_version[..n]).unwrap_or("<non-utf8>"))
                    .ok();
            }
            _ => {
                self.os_version_len = 0;
                std::log(
                    0,
                    "[SETTINGS APP] about.os_version not reported by the settings store; version unknown.",
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
// Visual Design Tokens (canonical light System Space palette)
// -----------------------------------------------------------------------------

const BG: u32 = std::design::color::BACKGROUND; // #F7F6F2 base
const SURFACE: u32 = std::design::color::SURFACE; // #FFFFFF card surface
const SOFT: u32 = std::design::color::SURFACE_SOFT; // #EFEEE9 controls / pills
const BORDER: u32 = std::design::color::BORDER; // #E1E0DB hairlines
const TEXT: u32 = std::design::color::TEXT; // #111214 primary text
const TEXT_MUTED: u32 = std::design::color::TEXT_MUTED; // #777773 secondary text
const ACCENT: u32 = std::design::color::ACCENT; // restrained blue
const SUCCESS_EMERALD: u32 = std::design::color::SUCCESS; // muted green
const GESTURE_BAR: u32 = 0xFFC4C3BD; // home indicator pill on the light base

// -----------------------------------------------------------------------------
// Layout constants (8-point grid; margins 24; touch targets >= 48)
// -----------------------------------------------------------------------------

/// Header height on the landing screen (back target + big title below).
const HEADER_BOTTOM: usize = 104;
/// Back button: 48x48 target, top-left, shared by both screens.
const BACK_X: usize = 24;
const BACK_Y: usize = 28;
const BACK_S: usize = 48;

/// Landing grid geometry: 2 columns of 328x136 cards, 16px gutters. The
/// height reserves two full caption lines so the sample's wrapping subtitles
/// ("Display, Sound, Gestures") are never sliced mid-phrase.
const CARD_W: usize = 328;
const CARD_H: usize = 136;
const CARD_R: usize = 18;
const GRID_X: usize = 24;
const GRID_Y: usize = 200;
const GRID_GAP: usize = 16;

/// Detail screen geometry: full-width stacked row cards.
const DETAIL_X: usize = 24;
const DETAIL_W: usize = 672;
const DETAIL_TOP: usize = 128;
const ROW_H: usize = 112;
const ROW_TALL: usize = 212; // DVFS row (selector + feedback line)
const ROW_TALL_ALT: usize = 140; // developer terminal row (two caption lines)
const ROW_GAP: usize = 16;
const DIVIDER_Y: usize = 104;

/// DVFS profile selector inside the Power detail (three 204x48 segments).
const SEG_X: usize = DETAIL_X + 24;
const SEG_W: usize = 204;
const SEG_GAP: usize = 6;
const SEG_H: usize = 48;

// -----------------------------------------------------------------------------
// Typography: measure -> fit -> draw. Nothing is ever clipped by its box.
// -----------------------------------------------------------------------------

/// Maximum bytes any single fitted run may occupy (source runs are shorter).
const RUN_CAP: usize = 96;

#[inline]
fn run_w(text: &[u8], scale: usize, bold: bool) -> usize {
    if bold {
        std::design::font::measure_bold(text, scale)
    } else {
        std::design::font::measure(text, scale)
    }
}

/// Copies `text` into `out`, appending an ellipsis when it does not fit
/// `max_w` at `scale` in the given weight. Returns the number of bytes
/// written (0 when even the ellipsis itself cannot fit, so the caller draws
/// nothing rather than a sliced word).
fn fit_run(text: &[u8], scale: usize, max_w: usize, bold: bool, out: &mut [u8]) -> usize {
    if run_w(text, scale, bold) <= max_w {
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        return n;
    }
    const DOTS: &[u8] = b"...";
    let dots_w = run_w(DOTS, scale, bold);
    if dots_w > max_w || out.len() < 3 {
        return 0;
    }
    let budget = max_w - dots_w;
    let mut used = 0usize;
    let mut fit = 0usize;
    for (i, &c) in text.iter().enumerate() {
        let adv = std::design::font::advance_of(c, bold) * scale;
        if used + adv > budget || fit + 3 > out.len() {
            break;
        }
        used += adv;
        fit = i + 1;
    }
    out[..fit].copy_from_slice(&text[..fit]);
    out[fit..fit + 3].copy_from_slice(DOTS);
    fit + 3
}

/// Anti-aliased text draw that (a) only tests pixels inside the run's own
/// line box, (b) fits the bytes to `max_w` with `fit_run` before touching the
/// glyph table, so a run can never paint outside the box the layout reserved
/// for it. `bg` is the surface under the text so the AA blend is correct.
#[inline(always)]
fn run_ink(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    let h = std::design::font::line_height(scale);
    if y < y0 || y >= y0 + h || x < x0 || x >= x0 + max_w {
        return None;
    }
    let mut buf = [0u8; RUN_CAP];
    let n = fit_run(text, scale, max_w, bold, &mut buf);
    if n == 0 {
        return None;
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, &buf[..n], color, bg, bold)
}

// -----------------------------------------------------------------------------
// Shape primitives (rounded rectangles, pills, chevron, category icons)
// -----------------------------------------------------------------------------

/// Whether `(x, y)` is inside the rounded rectangle `(x0, y0, w, h)`.
#[inline]
fn in_round_rect(x: usize, y: usize, x0: usize, y0: usize, w: usize, h: usize, r: usize) -> bool {
    if x < x0 || y < y0 || x >= x0 + w || y >= y0 + h {
        return false;
    }
    let r = r.min(w / 2).min(h / 2);
    if r == 0 {
        return true;
    }
    let left = x0 + r;
    let right = x0 + w - r;
    let top = y0 + r;
    let bottom = y0 + h - r;
    let cx = if x < left {
        left
    } else if x >= right {
        right - 1
    } else {
        x
    };
    let cy = if y < top {
        top
    } else if y >= bottom {
        bottom - 1
    } else {
        y
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    (dx * dx + dy * dy) as usize <= r * r
}

/// White card with a 1px hairline border; `None` outside the rounded rect.
#[inline]
fn card_pixel(x: usize, y: usize, x0: usize, y0: usize, w: usize, h: usize, r: usize) -> Option<u32> {
    if !in_round_rect(x, y, x0, y0, w, h, r) {
        return None;
    }
    if !in_round_rect(x, y, x0 + 1, y0 + 1, w - 2, h - 2, r.saturating_sub(1)) {
        return Some(BORDER);
    }
    Some(SURFACE)
}

/// Squared distance from `(x, y)` to the segment `a -> b`.
fn dist2_to_seg(x: usize, y: usize, ax: i64, ay: i64, bx: i64, by: i64) -> i64 {
    let px = x as i64;
    let py = y as i64;
    let abx = bx - ax;
    let aby = by - ay;
    let len2 = (abx * abx + aby * aby).max(1);
    let mut t = (px - ax) * abx + (py - ay) * aby;
    if t < 0 {
        t = 0;
    } else if t > len2 {
        t = len2;
    }
    let qx = ax + (t * abx) / len2;
    let qy = ay + (t * aby) / len2;
    (px - qx) * (px - qx) + (py - qy) * (py - qy)
}

/// Back chevron: one coherent 2px rounded stroke family, matching the header
/// control it lives in.
#[inline]
fn chevron_ink(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let (cx, cy) = (cx as i64, cy as i64);
    if dist2_to_seg(x, y, cx + 6, cy - 10, cx - 6, cy) <= 4
        || dist2_to_seg(x, y, cx - 6, cy, cx + 6, cy + 10) <= 4
    {
        Some(color)
    } else {
        None
    }
}

/// 48x48 back control: soft pill with the chevron centred.
fn back_button_pixel(x: usize, y: usize) -> Option<u32> {
    if !in_round_rect(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, 14) {
        return None;
    }
    if !in_round_rect(x, y, BACK_X + 1, BACK_Y + 1, BACK_S - 2, BACK_S - 2, 13) {
        return Some(BORDER);
    }
    if let Some(c) = chevron_ink(x, y, BACK_X + BACK_S / 2, BACK_Y + BACK_S / 2, TEXT) {
        return Some(c);
    }
    Some(SOFT)
}

// Category icon family: rounded geometric marks on a ~24px grid, 2px strokes,
// monochrome — the same coherent family the launcher's System Space uses.
#[inline]
fn icon_device(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    if dx >= -7 && dx <= 7 && dy >= -11 && dy <= 11 {
        let is_border = dx <= -6 || dx >= 6 || dy <= -10 || dy >= 10;
        let is_speaker = dy == -8 && dx >= -2 && dx <= 2;
        let is_home = dy == 8 && dx >= -3 && dx <= 3;
        if is_border || is_speaker || is_home {
            return Some(tint);
        }
    }
    None
}

#[inline]
fn icon_network(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    if dy <= 3 {
        if d2 >= 169 && d2 <= 289 && dx.abs() <= 15 {
            return Some(tint);
        }
        if d2 >= 49 && d2 <= 100 && dx.abs() <= 9 {
            return Some(tint);
        }
    }
    if d2 <= 9 && dy >= 0 {
        return Some(tint);
    }
    None
}

#[inline]
fn icon_personal(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    if d2 >= 121 && d2 <= 169 {
        return Some(tint);
    }
    let h_dy = dy + 4;
    if dx * dx + h_dy * h_dy <= 16 {
        return Some(tint);
    }
    if dy >= 3 && dy <= 10 && d2 < 121 {
        let shoulder_w = (11 - dy) * 2;
        if dx.abs() <= shoulder_w {
            return Some(tint);
        }
    }
    None
}

#[inline]
fn icon_apps(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let in_col1 = dx >= -9 && dx <= -3;
    let in_col2 = dx >= 3 && dx <= 9;
    let in_row1 = dy >= -9 && dy <= -3;
    let in_row2 = dy >= 3 && dy <= 9;
    if (in_col1 || in_col2) && (in_row1 || in_row2) {
        return Some(tint);
    }
    None
}

#[inline]
fn icon_battery(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    if dx >= -11 && dx <= 8 && dy >= -6 && dy <= 6 {
        if dx <= -10 || dx >= 7 || dy <= -5 || dy >= 5 {
            return Some(tint);
        }
        if dx <= 2 {
            return Some(tint);
        }
    }
    if dx >= 8 && dx <= 11 && dy >= -3 && dy <= 3 {
        return Some(tint);
    }
    None
}

#[inline]
fn icon_security(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    if dy >= -10 && dy <= 11 && dx.abs() <= 9 {
        let max_dx = if dy <= 0 {
            9
        } else {
            9 - (dy * 9) / 12
        };
        if dx.abs() <= max_dx {
            let is_edge = dx.abs() >= max_dx - 2 || dy <= -8 || dy >= 9;
            let is_keyhole = (dx.abs() <= 2 && dy >= -2 && dy <= 4)
                || (dx * dx + (dy + 3) * (dy + 3) <= 9);
            if is_edge || is_keyhole {
                return Some(tint);
            }
        }
    }
    None
}

#[inline]
fn icon_gear(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    if d2 <= 144 && d2 >= 25 {
        let is_spoke = dx.abs() <= 3
            || dy.abs() <= 3
            || (dx - dy).abs() <= 3
            || (dx + dy).abs() <= 3;
        let in_ring = d2 >= 25 && d2 <= 81;
        if in_ring || (is_spoke && d2 <= 144) {
            return Some(tint);
        }
    }
    None
}

type IconFn = fn(usize, usize, usize, usize, u32) -> Option<u32>;

/// Landing grid entry: title, subtitle and icon for one category.
fn category(i: usize) -> (&'static [u8], &'static [u8], IconFn) {
    match i {
        0 => (b"Device", b"Display, Sound, Gestures", icon_device),
        1 => (b"Network", b"Wi-Fi, Mobile, Bluetooth", icon_network),
        2 => (b"Personal", b"Accounts, Language, Time", icon_personal),
        3 => (b"Apps", b"Permissions, Storage", icon_apps),
        4 => (b"Power", b"Battery, Charging", icon_battery),
        5 => (b"Security", b"Lock, Encryption", icon_security),
        _ => (b"System", b"Updates, About, Tools", icon_gear),
    }
}

/// Top-left corner of landing card `i` (2-column grid, row-major).
fn card_origin(i: usize) -> (usize, usize) {
    let col = i % 2;
    let row = i / 2;
    (GRID_X + col * (CARD_W + GRID_GAP), GRID_Y + row * (CARD_H + GRID_GAP))
}

/// Landing card hit-test: the whole rounded card is the touch target.
fn landing_card_at(px: u32, py: u32) -> Option<Section> {
    for i in 0..Section::COUNT {
        let (cx0, cy0) = card_origin(i);
        if in_round_rect(px as usize, py as usize, cx0, cy0, CARD_W, CARD_H, CARD_R) {
            return Section::from_index(i);
        }
    }
    None
}

// -----------------------------------------------------------------------------
// Detail screen rows
// -----------------------------------------------------------------------------

/// Number of rows on each section's detail screen.
fn row_count(section: Section) -> usize {
    match section {
        Section::Apps => 2,
        Section::System => 4,
        _ => 3,
    }
}

/// Height of row `row` on `section`'s detail screen.
fn row_h(section: Section, row: usize) -> usize {
    match (section, row) {
        (Section::Power, 1) => ROW_TALL,
        (Section::System, 3) => ROW_TALL_ALT,
        _ => ROW_H,
    }
}

/// Top edge and height of row `row`, stacked from `DETAIL_TOP` with 16px gaps.
fn detail_row_rect(section: Section, row: usize) -> (usize, usize) {
    let mut y = DETAIL_TOP;
    for r in 0..row {
        y += row_h(section, r) + ROW_GAP;
    }
    (y, row_h(section, row))
}

/// Composes the About card's version line from the settings store's answer.
///
/// WHY: the card previously rendered a compile-time "Version: 0.8.2 ..."
/// literal; this builds the same line from `version` (capped at 16 bytes so
/// it cannot grow into the card badge) and falls back to an explicit
/// "not reported" line when the store did not answer.
fn compose_version_line(out: &mut [u8], version: &[u8]) -> usize {
    const PREFIX: &[u8] = b"Version: ";
    const SUFFIX: &[u8] = b" (x86_64 Sovereign Microkernel)";
    const UNKNOWN: &[u8] = b"Version: not reported by the settings store";
    if version.is_empty() {
        let n = UNKNOWN.len().min(out.len());
        out[..n].copy_from_slice(&UNKNOWN[..n]);
        return n;
    }
    let mut i = 0usize;
    for &b in PREFIX.iter().chain(version.iter().take(16)) {
        if i < out.len() {
            out[i] = b;
            i += 1;
        }
    }
    if i + SUFFIX.len() <= out.len() {
        for &b in SUFFIX {
            out[i] = b;
            i += 1;
        }
    }
    i
}

/// Writes `n` as decimal ASCII into `out`; returns the byte count.
fn write_decimal(n: u32, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if n == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    let mut v = n;
    while v > 0 && count < digits.len() {
        digits[count] = b'0' + (v % 10) as u8;
        count += 1;
        v /= 10;
    }
    let n_out = count.min(out.len());
    for i in 0..n_out {
        out[i] = digits[count - 1 - i];
    }
    n_out
}

/// Appends `suffix` to `buf[..len]` (truncating the suffix, never the buffer).
fn push_suffix(buf: &mut [u8], len: usize, suffix: &[u8]) -> usize {
    let mut i = len;
    for &b in suffix {
        if i >= buf.len() {
            break;
        }
        buf[i] = b;
        i += 1;
    }
    i
}

/// Pill badge, right-aligned helper: 28px pill, 1px ink border, label centred.
/// The width is derived from the measured label so the pill always fits its
/// text and the caller can reserve exactly that much horizontal space.
fn badge_w(label: &[u8]) -> usize {
    run_w(label, 1, false) + 24
}

fn badge_pixel(
    x: usize,
    y: usize,
    bx0: usize,
    by0: usize,
    label: &[u8],
    ink: u32,
    bg: u32,
) -> Option<u32> {
    let w = badge_w(label);
    let h = 28usize;
    if !in_round_rect(x, y, bx0, by0, w, h, h / 2) {
        return None;
    }
    if !in_round_rect(x, y, bx0 + 1, by0 + 1, w - 2, h - 2, h / 2 - 1) {
        return Some(ink);
    }
    let tw = run_w(label, 1, false);
    let tx = bx0 + (w - tw) / 2;
    let ty = by0 + (h - 24) / 2;
    if let Some(c) = run_ink(x, y, tx, ty, 1, label, tw + 1, ink, bg, false) {
        return Some(c);
    }
    Some(SOFT)
}

/// Standard row card head: bold title on its own line (never truncated by a
/// badge, because the title owns the full card width), caption below with the
/// badge column explicitly reserved, badge right-aligned on the caption line.
fn row_head(
    x: usize,
    y: usize,
    cy: usize,
    title: &[u8],
    caption: &[u8],
    cap_ink: u32,
    badge: &[u8],
    badge_ink: u32,
) -> Option<u32> {
    let bw = badge_w(badge);
    let bx0 = DETAIL_X + DETAIL_W - 24 - bw;
    let cap_fit = DETAIL_W - 48 - bw - 16;
    if let Some(c) = run_ink(
        x,
        y,
        DETAIL_X + 24,
        cy + 16,
        2,
        title,
        DETAIL_W - 48,
        TEXT,
        SURFACE,
        true,
    ) {
        return Some(c);
    }
    if let Some(c) = run_ink(
        x,
        y,
        DETAIL_X + 24,
        cy + 72,
        1,
        caption,
        cap_fit,
        cap_ink,
        SURFACE,
        false,
    ) {
        return Some(c);
    }
    badge_pixel(x, y, bx0, cy + 70, badge, badge_ink, SURFACE)
}

/// One segment of the DVFS profile selector (204x48, radius 12).
fn profile_segment_pixel(x: usize, y: usize, cy: usize, app: &SettingsApp, i: usize) -> Option<u32> {
    let sx = SEG_X + i * (SEG_W + SEG_GAP);
    if !in_round_rect(x, y, sx, cy + 112, SEG_W, SEG_H, 12) {
        return None;
    }
    let label: &[u8] = match i {
        0 => b"Performance",
        1 => b"Balanced",
        _ => b"PowerSaver",
    };
    let active = app.power_profile.index() == i;
    if !active
        && !in_round_rect(x, y, sx + 1, cy + 113, SEG_W - 2, SEG_H - 2, 11)
    {
        return Some(BORDER);
    }
    let fill = if active { TEXT } else { SOFT };
    let lw = run_w(label, 1, false);
    let lx = sx + (SEG_W - lw) / 2;
    let ly = cy + 112 + (SEG_H - 24) / 2;
    if let Some(c) = run_ink(
        x,
        y,
        lx,
        ly,
        1,
        label,
        SEG_W - 16,
        if active { SURFACE } else { TEXT_MUTED },
        fill,
        false,
    ) {
        return Some(c);
    }
    Some(fill)
}

/// Renders the contents of one detail row for the pixel under test.
#[allow(clippy::too_many_arguments)]
fn row_pixel(
    x: usize,
    y: usize,
    app: &SettingsApp,
    section: Section,
    row: usize,
    cy: usize,
    fill: u32,
) -> u32 {
    let _ = fill; // rows are drawn on the white card surface
    match section {
        Section::Device => match row {
            0 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Display",
                    b"No backend attached",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
            1 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Sound",
                    b"No backend attached",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
            _ => {
                let reported = app.sensor == TelemetryState::Reported;
                let (caption, cap_ink, badge, badge_ink) = if reported {
                    (
                        &b"Sensor payload received; values not parsed"[..],
                        TEXT_MUTED,
                        &b"REPORTED"[..],
                        SUCCESS_EMERALD,
                    )
                } else {
                    (
                        &b"Sensor values unavailable"[..],
                        TEXT_MUTED,
                        &b"UNAVAILABLE"[..],
                        TEXT_MUTED,
                    )
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Motion Sensor",
                    caption,
                    cap_ink,
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
            }
        },
        Section::Network => match row {
            0 => {
                let reported = app.wireless == TelemetryState::Reported;
                let (caption, badge, badge_ink) = if reported {
                    (
                        &b"Wireless telemetry received"[..],
                        &b"REPORTED"[..],
                        SUCCESS_EMERALD,
                    )
                } else {
                    (
                        &b"Radio status unavailable"[..],
                        &b"UNAVAILABLE"[..],
                        TEXT_MUTED,
                    )
                };
                // WHY no Wi-Fi toggle: the platform exposes no radio-control
                // syscall to this app, and the design truth contract forbids
                // dead controls, so the row reports the observed radio state.
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Wi-Fi Radio",
                    caption,
                    if reported { SUCCESS_EMERALD } else { TEXT_MUTED },
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
            }
            1 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Bluetooth",
                    b"Bluetooth status unavailable",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
            _ => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Cellular Baseband",
                    b"Cellular status unavailable",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
        },
        Section::Personal => {
            let title: &[u8] = match row {
                0 => b"Accounts",
                1 => b"Language",
                _ => b"Date & Time",
            };
            if let Some(c) = row_head(
                x,
                y,
                cy,
                title,
                b"No backend attached",
                TEXT_MUTED,
                b"UNAVAILABLE",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        Section::Apps => {
            let title: &[u8] = match row {
                0 => b"Permissions",
                _ => b"Storage",
            };
            if let Some(c) = row_head(
                x,
                y,
                cy,
                title,
                b"No backend attached",
                TEXT_MUTED,
                b"UNAVAILABLE",
                TEXT_MUTED,
            ) {
                return c;
            }
        }
        Section::Power => match row {
            0 => {
                let reported = app.battery == TelemetryState::Reported;
                let (caption, badge, badge_ink) = if reported {
                    (
                        &b"Battery payload received"[..],
                        &b"REPORTED"[..],
                        SUCCESS_EMERALD,
                    )
                } else {
                    (
                        &b"Battery telemetry unavailable"[..],
                        &b"UNAVAILABLE"[..],
                        TEXT_MUTED,
                    )
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Battery Fuel Gauge",
                    caption,
                    TEXT_MUTED,
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
            }
            1 => {
                // CPU frequency scaling: observed governor (real query) plus a
                // live DVFS profile selector whose result is reported honestly.
                let (gov_run, gov_ink): (&[u8], u32) = match app.governor {
                    Some(0) => (b"Governor reported by the kernel: Performance", TEXT_MUTED),
                    Some(1) => (b"Governor reported by the kernel: Power Saver", TEXT_MUTED),
                    Some(2) => (b"Governor reported by the kernel: Interactive", TEXT_MUTED),
                    Some(3) => (b"Governor reported by the kernel: Schedutil", TEXT_MUTED),
                    _ => (b"DVFS state unavailable", TEXT_MUTED),
                };
                let gov_reported = app.governor.is_some();
                let (gov_badge, gov_badge_ink): (&[u8], u32) = if gov_reported {
                    (b"REPORTED", SUCCESS_EMERALD)
                } else {
                    (b"UNAVAILABLE", TEXT_MUTED)
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"CPU Frequency Scaling",
                    gov_run,
                    gov_ink,
                    gov_badge,
                    gov_badge_ink,
                ) {
                    return c;
                }
                for i in 0..3 {
                    if let Some(c) = profile_segment_pixel(x, y, cy, app, i) {
                        return c;
                    }
                }
                // Honest outcome line for the last profile request.
                let mut fb = [0u8; 96];
                let (fb_len, fb_ink) = match app.profile_feedback {
                    ProfileFeedback::Applied(mhz) => {
                        let mut i = write_decimal(mhz, &mut fb);
                        i = push_suffix(&mut fb, i, b" MHz accepted by the kernel");
                        (i, ACCENT)
                    }
                    ProfileFeedback::Refused => {
                        let s = b"Kernel refused the change; no profile applied";
                        fb[..s.len()].copy_from_slice(s);
                        (s.len(), TEXT_MUTED)
                    }
                    ProfileFeedback::Unattempted => {
                        let s = b"Profile not yet sent to the kernel";
                        fb[..s.len()].copy_from_slice(s);
                        (s.len(), TEXT_MUTED)
                    }
                };
                if let Some(c) = run_ink(
                    x,
                    y,
                    DETAIL_X + 24,
                    cy + 172,
                    1,
                    &fb[..fb_len],
                    DETAIL_W - 48,
                    fb_ink,
                    SURFACE,
                    false,
                ) {
                    return c;
                }
            }
            _ => {
                let reported = app.thermal == TelemetryState::Reported;
                let (caption, badge, badge_ink) = if reported {
                    (
                        &b"Thermal payload received"[..],
                        &b"REPORTED"[..],
                        SUCCESS_EMERALD,
                    )
                } else {
                    (
                        &b"Temperature telemetry unavailable"[..],
                        &b"UNAVAILABLE"[..],
                        TEXT_MUTED,
                    )
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Thermal Governor",
                    caption,
                    TEXT_MUTED,
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
            }
        },
        Section::Security => match row {
            0 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Hardware Keystore",
                    b"Keystore status unavailable", TEXT_MUTED,
                    b"UNAVAILABLE", TEXT_MUTED,
                ) {
                    return c;
                }
            }
            1 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Storage Vault Encryption",
                    b"Vault encryption status unavailable", TEXT_MUTED,
                    b"UNAVAILABLE", TEXT_MUTED,
                ) {
                    return c;
                }
            }
            _ => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Privacy Shield Filter",
                    b"Privacy filter status unavailable", TEXT_MUTED,
                    b"UNAVAILABLE", TEXT_MUTED,
                ) {
                    return c;
                }
            }
        },
        Section::System => match row {
            0 => {
                // Version line built from the settings store's answer.
                let mut ver = [0u8; 64];
                let n = compose_version_line(
                    &mut ver,
                    &app.os_version[..app.os_version_len],
                );
                let reported = app.os_version_len > 0;
                let (badge, badge_ink) = if reported {
                    (&b"REPORTED"[..], SUCCESS_EMERALD)
                } else {
                    (&b"UNAVAILABLE"[..], TEXT_MUTED)
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"InterOS Core",
                    &ver[..n],
                    if reported { TEXT } else { TEXT_MUTED },
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
            }
            1 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Platform Microkernel",
                    b"Capability Security & VFS Block Storage",
                    TEXT_MUTED,
                    b"RING 0",
                    ACCENT,
                ) {
                    return c;
                }
            }
            2 => {
                let reported = app.rtc == TelemetryState::Reported;
                let mut rbuf = [0u8; 80];
                let n = if reported {
                    let src = &app.rtc_time[..app.rtc_time_len];
                    let n = src.len().min(rbuf.len());
                    rbuf[..n].copy_from_slice(&src[..n]);
                    n
                } else {
                    let s = b"No RTC reading (query failed or was refused)";
                    rbuf[..s.len()].copy_from_slice(s);
                    s.len()
                };
                let (badge, badge_ink) = if reported {
                    (&b"REPORTED"[..], SUCCESS_EMERALD)
                } else {
                    (&b"UNAVAILABLE"[..], TEXT_MUTED)
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Hardware Real-Time Clock",
                    &rbuf[..n],
                    if reported { ACCENT } else { TEXT_MUTED },
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
            }
            _ => {
                // Developer terminal: a real (capability-refused) mutator, so
                // the row stays tappable; the badge reports the last state the
                // kernel actually persisted.
                let (state_run, badge, badge_ink) = if app.terminal_enabled {
                    (&b"Enabled"[..], &b"ENABLED"[..], SUCCESS_EMERALD)
                } else {
                    (
                        &b"Disabled by default"[..],
                        &b"DISABLED"[..],
                        TEXT_MUTED,
                    )
                };
                let bw = badge_w(badge);
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Developer terminal",
                    state_run,
                    TEXT_MUTED,
                    badge,
                    badge_ink,
                ) {
                    return c;
                }
                if let Some(c) = run_ink(
                    x,
                    y,
                    DETAIL_X + 24,
                    cy + 98,
                    1,
                    b"Config key: developer.terminal_enabled",
                    DETAIL_W - 48 - bw - 16,
                    TEXT_MUTED,
                    SURFACE,
                    false,
                ) {
                    return c;
                }
            }
        },
    }
    SURFACE
}

/// Detail screen pixel: header (back + title + divider) then the stacked rows.
fn detail_pixel(x: usize, y: usize, app: &SettingsApp, section: Section) -> u32 {
    if let Some(c) = run_ink(
        x,
        y,
        BACK_X + BACK_S + 16,
        BACK_Y,
        2,
        section.title(),
        W - (BACK_X + BACK_S + 16) - 24,
        TEXT,
        BG,
        true,
    ) {
        return c;
    }
    if y == DIVIDER_Y {
        return BORDER;
    }
    for row in 0..row_count(section) {
        let (cy, ch) = detail_row_rect(section, row);
        if let Some(fill) = card_pixel(x, y, DETAIL_X, cy, DETAIL_W, ch, CARD_R) {
            return row_pixel(x, y, app, section, row, cy, fill);
        }
    }
    BG
}

/// One landing category card: icon, bold title, subtitle wrapped to at most
/// two measured lines (wrapping, not ellipsis, because the phrases are short
/// word lists that must stay whole).
fn landing_card_pixel(x: usize, y: usize, i: usize) -> Option<u32> {
    let (cx0, cy0) = card_origin(i);
    if let Some(fill) = card_pixel(x, y, cx0, cy0, CARD_W, CARD_H, CARD_R) {
        let (title, subtitle, icon) = category(i);
        if let Some(ic) = icon(x, y, cx0 + 44, cy0 + 48, TEXT) {
            return Some(ic);
        }
        // Title owns the full remaining card width; subtitle below it.
        if let Some(c) = run_ink(
            x,
            y,
            cx0 + 80,
            cy0 + 22,
            2,
            title,
            CARD_W - 104,
            TEXT,
            fill,
            true,
        ) {
            return Some(c);
        }
        let mut lines: [&[u8]; 2] = [&b""[..], &b""[..]];
        let n = std::design::font::wrap_text(subtitle, 1, CARD_W - 104, 2, &mut lines);
        for li in 0..n {
            if let Some(c) = run_ink(
                x,
                y,
                cx0 + 80,
                cy0 + 78 + li * 24,
                1,
                lines[li],
                CARD_W - 104,
                TEXT_MUTED,
                fill,
                false,
            ) {
                return Some(c);
            }
        }
        return Some(fill);
    }
    None
}

/// Landing screen pixel: header back target, big title, hero line, grid.
fn landing_pixel(x: usize, y: usize) -> u32 {
    if y >= HEADER_BOTTOM {
        for i in 0..Section::COUNT {
            if let Some(c) = landing_card_pixel(x, y, i) {
                return c;
            }
        }
    }
    if let Some(c) = run_ink(x, y, GRID_X, 96, 2, b"System Space", W - 48, TEXT, BG, true) {
        return c;
    }
    if let Some(c) = run_ink(
        x,
        y,
        GRID_X,
        152,
        1,
        b"Everything in one place",
        W - 48,
        TEXT_MUTED,
        BG,
        false,
    ) {
        return c;
    }
    BG
}

#[inline(never)]
fn get_settings_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    // Home gesture pill (matches the touch handler's exit region).
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return GESTURE_BAR;
    }
    // Shared 48x48 back control, top-left of both screens.
    if let Some(c) = back_button_pixel(x, y) {
        return c;
    }
    match app.current_section {
        None => landing_pixel(x, y),
        Some(section) => detail_pixel(x, y, app, section),
    }
}

// -----------------------------------------------------------------------------
// Presentation & event loop
// -----------------------------------------------------------------------------

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

/// Hit-test for the DVFS profile selector segments on the Power detail.
fn profile_segment_at(px: u32, py: u32) -> Option<usize> {
    let (cy, _) = detail_row_rect(Section::Power, 1);
    for i in 0..3 {
        let sx = SEG_X + i * (SEG_W + SEG_GAP);
        if in_round_rect(px as usize, py as usize, sx, cy + 112, SEG_W, SEG_H, 12) {
            return Some(i);
        }
    }
    None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[SETTINGS APP (Ring 3)] System Space (Sovereign Settings & Diagnostics) online.",
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

    // 1. Inspect Wireless Configuration (Network detail)
    settings.refresh_wireless();

    // 2. Inspect Power & Battery State (Power detail)
    settings.refresh_power();

    // 3. Inspect Thermals & Hardware Sensors (Power + Device details)
    settings.refresh_thermals_and_sensors();

    // 4. Audit Security & Hardware Keystore (Security detail)
    settings.audit_security();

    // 5. Query About Device & Hardware RTC (System detail)
    settings.display_about_device();
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
                        // Back control: pop to the landing grid, or exit the
                        // app when already on the landing grid. Home gesture
                        // bar always exits to the launcher.
                        if in_round_rect(
                            input.x as usize,
                            input.y as usize,
                            BACK_X,
                            BACK_Y,
                            BACK_S,
                            BACK_S,
                            14,
                        ) {
                            match settings.current_section {
                                Some(_) => {
                                    settings.current_section = None;
                                    std::log(
                                        0,
                                        "[SETTINGS APP] Back navigation: returned to the System Space landing grid.",
                                    )
                                    .ok();
                                }
                                None => {
                                    std::log(
                                        0,
                                        "[SETTINGS APP] Back button pressed on the root screen. Exiting to launcher.",
                                    )
                                    .ok();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(6, surface);
                                    }
                                    std::exit_with_code(0);
                                }
                            }
                        } else if input.x >= 240 && input.x <= 480 && input.y >= 1400 {
                            std::log(
                                0,
                                "[SETTINGS APP] Home gesture pressed. Exiting to launcher.",
                            )
                            .ok();
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::exit_with_code(0);
                        } else if let Some(section) = settings.current_section {
                            // Detail-screen controls (hit cells match the drawn
                            // geometry exactly).
                            match section {
                                Section::Power => {
                                    if let Some(i) = profile_segment_at(input.x, input.y) {
                                        let profile = match i {
                                            0 => PowerProfile::Performance,
                                            1 => PowerProfile::Balanced,
                                            _ => PowerProfile::PowerSaver,
                                        };
                                        settings.apply_power_profile(profile);
                                    }
                                }
                                Section::System => {
                                    let (cy, ch) = detail_row_rect(Section::System, 3);
                                    if in_round_rect(
                                        input.x as usize,
                                        input.y as usize,
                                        DETAIL_X,
                                        cy,
                                        DETAIL_W,
                                        ch,
                                        CARD_R,
                                    ) {
                                        settings.toggle_terminal();
                                    }
                                }
                                _ => {}
                            }
                        } else if let Some(section) = landing_card_at(input.x, input.y) {
                            settings.current_section = Some(section);
                            std::log(
                                0,
                                "[SETTINGS APP] System Space category opened.",
                            )
                            .ok();
                        }
                        if surface > 0 {
                            render_settings_ui(surface, &settings);
                            let _ = std::surface_flip(6);
                        }
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
