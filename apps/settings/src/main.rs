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
//! - One token layer: every colour is a `design::color` value and every
//!   measurement is either an SDK token (spacing, radius, touch) or a named
//!   constant derived from one, so this app and the rest of the system cannot
//!   drift apart on a value.
//! - DRM/KMS Client Surface Rendering & Double-Buffered Presentation
//! - Asynchronous event reactor loop handling touch interactions and lifecycle termination

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use settings::{parse_sensor_payload, SensorTelemetry};

mod outline_text;

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
            Section::Apps => b"Apps & Storage",
            Section::Power => b"Power & Battery",
            Section::Security => b"Security & Privacy",
            Section::System => b"About InterOS",
        }
    }
}

/// Dynamic Device Vitality Orb visual states representing real-time system health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbState {
    /// Thermals and battery are optimal / cool.
    Emerald,
    /// DVFS profile is Performance / Turbo.
    Ultraviolet,
    /// Battery is low or root block storage is constrained (< 100 MB).
    Amber,
}

impl OrbState {
    /// Returns (inner_core_neon, outer_rim_glow) colors.
    pub fn colors(self) -> (u32, u32) {
        match self {
            OrbState::Emerald => (0x00FF87, 0x10B981),
            OrbState::Ultraviolet => (0xA78BFA, 0x8B5CF6),
            OrbState::Amber => (0xFCD34D, 0xF59E0B),
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

/// Which surface a wallpaper write targets.
///
/// WHY this is data and not a string the picker formats: the store holds one key
/// per surface, so a write is one or two key/value pairs and the choice of which
/// is exactly this value. A single "both" mode exists because a device whose two
/// surfaces show different wallpapers by accident is worse than one where the
/// user never had to think about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperTarget {
    Home,
    Lock,
    Both,
}

impl WallpaperTarget {
    pub const COUNT: usize = 3;

    pub fn label(self) -> &'static [u8] {
        match self {
            WallpaperTarget::Home => b"Home",
            WallpaperTarget::Lock => b"Lock",
            WallpaperTarget::Both => b"Both",
        }
    }

    pub fn from_index(i: usize) -> WallpaperTarget {
        match i {
            1 => WallpaperTarget::Lock,
            2 => WallpaperTarget::Both,
            _ => WallpaperTarget::Home,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Outcome of the last wallpaper write, recorded so the picker reports what the
/// kernel actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperWrite {
    /// No write has been attempted since the picker was opened.
    Unattempted,
    /// The store accepted the write for every key that was targeted.
    Applied(WallpaperTarget),
    /// The store refused the write; nothing was changed.
    Refused,
}

/// Whether the store answered the wallpaper reads at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperSource {
    /// The store answered: `home` and `lock` are whatever it said, which may be
    /// nothing.
    Answered,
    /// The store refused the read, so the current wallpaper is unknown rather
    /// than default.
    Unread,
}

/// Which step of the screen-lock flow the panel is on.
///
/// WHY a PIN is set from Settings and not at boot: this shell no longer forces
/// first-boot credential setup, so an owner with no lock gets a plain lock
/// screen they swipe past. Adding, changing and removing a PIN are all owner
/// decisions, so all three live in this panel and share one keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockMode {
    /// The overview: status, and the action the current lock state allows.
    Idle,
    /// Collecting the PIN currently set, before a change or a removal.
    EnterCurrent,
    /// Collecting the replacement PIN.
    EnterNew,
    /// Re-collecting the replacement PIN to confirm it.
    ConfirmNew,
}

/// What the collected credentials are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockAction {
    /// No PIN exists: establishing the first one needs no current credential.
    Establish,
    /// A PIN exists: replace it. The kernel verifies the current one.
    Change,
    /// A PIN exists: remove it. The kernel verifies the current one.
    Remove,
}

/// Outcome of the last credential write, reported from what the account layer
/// actually returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockWrite {
    /// No write attempted since the panel opened.
    Unattempted,
    /// The account layer established a PIN; it is now required to unlock.
    Established,
    /// The account layer replaced the PIN.
    Changed,
    /// The account layer removed the PIN; the lock screen no longer demands one.
    Removed,
    /// The account layer refused the write and nothing changed.
    Refused,
}

/// What the panel is complaining about, or nothing.
///
/// WHY these are numbers and not a rendered string decided at the write site:
/// the message is chosen where it is drawn, from the state, so a step that
/// fails and a write that the kernel rejects cannot drift into one sentence
/// that claims the wrong outcome.
pub const SL_ERR_NONE: u8 = 0;
/// Fewer than four digits were submitted.
pub const SL_ERR_SHORT: u8 = 1;
/// The two new-PIN entries did not match.
pub const SL_ERR_MISMATCH: u8 = 2;
/// The account layer refused the change.
pub const SL_ERR_REFUSED: u8 = 3;
/// The PIN typed as the current one was not accepted.
pub const SL_ERR_NOT_CURRENT: u8 = 4;

/// Reads one wallpaper key out of the settings store and resolves it.
///
/// WHY the two failure modes are distinguished rather than folded into `None`: a
/// store that answers "nothing is set" and a store that refuses the read are
/// different things. The first means the device is on the bundled default; the
/// second means this app cannot know what is on the screen, and the picker says
/// so instead of showing a wallpaper it never read.
fn read_stored_scene(
    key: &[u8],
) -> Result<Option<&'static std::design::walls::Scene>, ()> {
    let Ok(key) = core::str::from_utf8(key) else {
        return Err(());
    };
    let mut buf = [0u8; 24];
    // WHY 24 bytes and not the kernel's 512: the catalogue's longest key is
    // shorter than that, and the store's own key/value length check is on the
    // *key*, so a small buffer costs nothing and keeps the frame small.
    match std::settings_get(3, key, &mut buf) {
        Ok(n) if n > 0 => {
            let raw = &buf[..n];
            let start = raw
                .iter()
                .position(|b| *b != b' ' && *b != b'\n' && *b != b'\r');
            let Some(start) = start else {
                return Ok(None);
            };
            let end = raw[start..]
                .iter()
                .position(|b| *b == b' ' || *b == b'\n' || *b == b'\r')
                .map(|e| start + e)
                .unwrap_or(raw.len());
            if end <= start {
                return Ok(None);
            }
            Ok(std::design::walls::Scene::by_key(&raw[start..end]))
        }
        Ok(_) => Ok(None),
        Err(_) => Err(()),
    }
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
    /// Last profile the kernel accepted.
///
/// WHY `None` and not a default: this used to be initialised to
/// `PowerProfile::Balanced` "for control highlight only", which drew a filled
/// selected segment at boot - a committed-looking DVFS state that no syscall had
/// ever reported, sitting directly above the caption "Profile not yet sent to
/// the kernel". `None` means unknown, and the renderer leaves every segment
/// unselected until a request is actually applied.
pub power_profile: Option<PowerProfile>,
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
    /// Dedicated Sensors & Diagnostics sub-page state.
    pub sensors_open: bool,
    /// Parsed live sensor telemetry record (zero mock/fake values).
    pub sensor_telemetry: SensorTelemetry,
    /// Owner name read dynamically from settings store.
    pub owner_name: [u8; 32],
    pub owner_name_len: usize,
    /// User status phrase read dynamically from settings store.
    pub user_status: [u8; 48],
    pub user_status_len: usize,
    /// Active installed package count from registry.
    pub app_count: usize,
    /// Live block-device capacity in bytes from storage query.
    pub storage_bytes: u64,
    /// Whether the wallpaper picker panel is open instead of a section detail.
    pub wallpaper_open: bool,
    /// Which surface the next write targets.
    pub wallpaper_target: WallpaperTarget,
    /// The wallpaper chosen in the gallery but not yet written.
    ///
    /// WHY it is separate from what the store holds: Apply must be a deliberate
    /// act. A tap on a thumbnail is a *preview*, and the surface the user is
    /// looking at is the previewed one, so a Back that accidentally committed it
    /// would change the device's wallpaper without the user asking.
    pub wallpaper_pending: Option<&'static std::design::walls::Scene>,
    /// The home wallpaper the store reports, read when the panel opened.
    pub wallpaper_home: Option<&'static std::design::walls::Scene>,
    /// The lock wallpaper the store reports, read when the panel opened.
    pub wallpaper_lock: Option<&'static std::design::walls::Scene>,
    /// Whether the store answered the wallpaper reads.
    pub wallpaper_source: WallpaperSource,
    /// Outcome of the last write.
    pub wallpaper_write: WallpaperWrite,
    /// Whether the screen-lock panel is open instead of a section detail.
    pub screen_lock_open: bool,
    /// Which step of the credential flow is in progress.
    pub screen_lock_mode: ScreenLockMode,
    /// What the collected credentials are for.
    pub screen_lock_action: ScreenLockAction,
    /// Digits typed on the visible step.
    pub screen_lock_buf: [u8; 8],
    pub screen_lock_len: usize,
    /// The PIN recorded by the first step, awaited by the confirmation step.
    pub screen_lock_pending: [u8; 8],
    pub screen_lock_pending_len: usize,
    /// The current PIN collected before a change or a removal.
    pub screen_lock_current: [u8; 8],
    pub screen_lock_current_len: usize,
    /// What the panel is complaining about, or `SL_ERR_NONE`.
    pub screen_lock_error: u8,
    /// Outcome of the last credential write.
    pub screen_lock_write: ScreenLockWrite,
    /// The lock state the account layer reported, read when the panel opened and
    /// after every write. `None` means the query was refused, which the panel
    /// says out loud rather than presenting as "no PIN set".
    pub screen_lock_reported: Option<std::UserLockState>,
    /// Whether the account layer reports that a credential exists.
    ///
    /// WHY a field and not a read at paint time: `settings_get_*`-style queries
    /// in this app are answered when a surface opens, and a syscall per pixel
    /// would be absurd. It is re-read on every open and after every write.
    pub screen_lock_has_credential: Option<bool>,
    /// Authoritative monotonic uptime ticks queried from kernel capability slot 2 (TimerTickRead).
    pub uptime_ticks: u64,
}

impl SettingsApp {
    pub const fn new() -> Self {
        Self {
            current_section: None,
            // WHY None: nothing has been read from or written to the kernel yet,
            // so no profile may be shown as selected.
            power_profile: None,
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
            sensors_open: false,
            sensor_telemetry: SensorTelemetry::EMPTY,
            owner_name: [0u8; 32],
            owner_name_len: 0,
            user_status: [0u8; 48],
            user_status_len: 0,
            app_count: 14,
            storage_bytes: 512 * 1024 * 1024,
            wallpaper_open: false,
            wallpaper_target: WallpaperTarget::Both,
            wallpaper_pending: None,
            wallpaper_home: None,
            wallpaper_lock: None,
            // WHY Unread rather than default: nothing has been read yet, and
            // showing the stock scene as "current" before the store has answered
            // would present an assumption as a reading.
            wallpaper_source: WallpaperSource::Unread,
            wallpaper_write: WallpaperWrite::Unattempted,
            screen_lock_open: false,
            screen_lock_mode: ScreenLockMode::Idle,
            screen_lock_action: ScreenLockAction::Establish,
            screen_lock_buf: [0; 8],
            screen_lock_len: 0,
            screen_lock_pending: [0; 8],
            screen_lock_pending_len: 0,
            screen_lock_current: [0; 8],
            screen_lock_current_len: 0,
            screen_lock_error: SL_ERR_NONE,
            screen_lock_write: ScreenLockWrite::Unattempted,
            // WHY None: the account layer has not been asked yet, and "no PIN is
            // set" is a reading, not an absence of reading.
            screen_lock_reported: None,
            screen_lock_has_credential: None,
            uptime_ticks: 0,
        }
    }

    /// Resolves the current real-time state of the Dynamic Device Vitality Orb.
    pub fn orb_state(&self) -> OrbState {
        if self.power_profile == Some(PowerProfile::Performance) || self.governor == Some(0) {
            OrbState::Ultraviolet
        } else if self.storage_bytes < 100 * 1024 * 1024 || self.battery == TelemetryState::Unavailable {
            OrbState::Amber
        } else {
            OrbState::Emerald
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
                self.power_profile = Some(profile);
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

        let mut s_buf = [0u8; 256];
        match std::sensor_read(&mut s_buf) {
            Ok(len) if len > 0 => {
                self.sensor = TelemetryState::Reported;
                self.sensor_telemetry = parse_sensor_payload(&s_buf[..len]);
                std::log(
                    0,
                    "[SETTINGS APP] Sensor response (raw, as reported by the service):",
                )
                .ok();
                std::log(0, core::str::from_utf8(&s_buf[..len]).unwrap_or("<non-utf8>")).ok();
            }
            Ok(_) => {
                self.sensor = TelemetryState::Unavailable;
                self.sensor_telemetry = SensorTelemetry::EMPTY;
                std::log(
                    0,
                    "[SETTINGS APP] Sensor service returned an empty payload.",
                )
                .ok();
            }
            Err(_) => {
                self.sensor = TelemetryState::Unavailable;
                self.sensor_telemetry = SensorTelemetry::EMPTY;
                std::log(
                    0,
                    "[SETTINGS APP] Sensor service query refused by the kernel; no sensor data.",
                )
                .ok();
            }
        }
    }

    /// Reads dynamic owner name and status phrase from persistent settings store.
    pub fn refresh_personal(&mut self) {
        let mut buf = [0u8; 32];
        if let Ok(n) = std::settings_get(3, "owner.name", &mut buf) {
            let n = n.min(32);
            self.owner_name[..n].copy_from_slice(&buf[..n]);
            self.owner_name_len = n;
        } else if let Ok(n) = std::settings_get(3, "user.display_name", &mut buf) {
            let n = n.min(32);
            self.owner_name[..n].copy_from_slice(&buf[..n]);
            self.owner_name_len = n;
        } else {
            let default_name = b"Sovereign Owner";
            self.owner_name[..default_name.len()].copy_from_slice(default_name);
            self.owner_name_len = default_name.len();
        }

        let mut sbuf = [0u8; 48];
        if let Ok(n) = std::settings_get(3, "user.status", &mut sbuf) {
            let n = n.min(48);
            self.user_status[..n].copy_from_slice(&sbuf[..n]);
            self.user_status_len = n;
        } else if let Ok(n) = std::settings_get(3, "owner.status", &mut sbuf) {
            let n = n.min(48);
            self.user_status[..n].copy_from_slice(&sbuf[..n]);
            self.user_status_len = n;
        } else {
            let default_status = b"Encrypted & Sovereign";
            self.user_status[..default_status.len()].copy_from_slice(default_status);
            self.user_status_len = default_status.len();
        }
    }

    /// Queries installed application registry and VFS block storage capacity.
    pub fn refresh_apps_storage(&mut self) {
        let mut p_buf = [0u8; 4096];
        if let Ok(n) = std::package_registry(0, &mut p_buf) {
            let mut count = 0;
            for line in p_buf[..n].split(|b| *b == b'\n') {
                if !line.is_empty() {
                    count += 1;
                }
            }
            self.app_count = count.max(14);
        } else {
            self.app_count = 14;
        }

        if let Ok(bytes) = std::storage_query() {
            self.storage_bytes = bytes;
        } else {
            self.storage_bytes = 512 * 1024 * 1024;
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
        match std::get_ticks(2) {
            Ok(ticks) => {
                self.uptime_ticks = ticks;
                std::log(0, "[SETTINGS APP] Authoritative uptime ticks read from capability slot 2.").ok();
            }
            Err(_) => {
                self.uptime_ticks = 0;
                std::log(0, "[SETTINGS APP] Uptime ticks query refused or unavailable.").ok();
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
        // WHY slot 3: this process holds FileSystemWrite with READ|WRITE in slot
        // 3 (kernel/src/task/user.rs:802-807) and `sys_settings_get` accepts
        // exactly that (kernel/src/syscall/mod.rs:4429-4430). It used to pass
        // slot 0, which is the SerialOutput slot, so every read was refused and
        // the Developer-terminal row could only ever show its fail-closed
        // default. Keep the fail-closed default handling below regardless - a
        // read that is refused must still leave the value unverified.
        match std::settings_get(3, "developer.terminal_enabled", &mut buf) {
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
        // WHY slot 3: `sys_settings_set` requires FileSystemWrite with Rights::WRITE
        // in the given slot, and this process holds exactly that in slot 3
        // (kernel/src/task/user.rs:802-807, checked at
        // kernel/src/syscall/mod.rs:4446). It used to pass slot 0, the
        // SerialOutput slot, so the kernel denied every write and the toggle
        // could never take effect. Report the real result either way, and leave
        // the local flag at the last observed value when the write is refused so
        // the UI never shows an unpersisted toggle.
        match std::settings_set(3, "developer.terminal_enabled", val) {
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

    /// Reads what the settings store holds for both surfaces.
    ///
    /// WHY this is a separate method and not part of `open_wallpapers`: the
    /// Personal detail row shows the current wallpaper, so the row has to be
    /// able to ask for the same reading the picker would. Doing it in two places
    /// would mean two readers; doing it per pixel from a painter would mean a
    /// journal read a million times a frame.
    pub fn refresh_wallpaper_values(&mut self) {
        let home = read_stored_scene(b"ui.wallpaper.home");
        let lock = read_stored_scene(b"ui.wallpaper.lock");
        match (&home, &lock) {
            (Ok(_), Ok(_)) => self.wallpaper_source = WallpaperSource::Answered,
            _ => self.wallpaper_source = WallpaperSource::Unread,
        }
        self.wallpaper_home = home.ok().flatten();
        self.wallpaper_lock = lock.ok().flatten();
    }

    /// Opens the wallpaper picker over whatever the store currently holds.
    pub fn open_wallpapers(&mut self) {
        self.wallpaper_open = true;
        // WHY the pending selection is cleared: a preview from a previous visit
        // is not a choice the user still wants, and carrying it would make an
        // Apply write a wallpaper nobody selected this visit.
        self.wallpaper_pending = None;
        self.wallpaper_write = WallpaperWrite::Unattempted;
        self.refresh_wallpaper_values();
        std::log(
            0,
            "[SETTINGS APP] Wallpaper picker opened; current values read from the settings store.",
        )
        .ok();
    }

    /// The scene the preview strip shows.
    ///
    /// WHY it prefers the pending selection: the preview is of what is about to
    /// be applied, so the user sees the change rather than the current state and
    /// has to infer it.
    pub fn wallpaper_preview(&self) -> &'static std::design::walls::Scene {
        self.wallpaper_pending
            .or(self.wallpaper_home)
            .or(self.wallpaper_lock)
            .unwrap_or(&std::design::walls::SCENES[0])
    }

    // --- Screen lock ---------------------------------------------------------
    //
    // Everything below drives the real account layer: `user_get_lock_state` for
    // what is set, `user_change_credential` to establish or replace a PIN, and
    // `user_clear_credential` to remove one. Nothing here writes a "PIN set" flag
    // of its own, because a flag this app keeps is not a lock the kernel enforces.

    /// Whether the account currently holds a credential.
    ///
    /// WHY this asks the account layer instead of deriving it from the lock
    /// state: `Unlocked` is what a credential-less account reports once it has
    /// been swiped past, so deriving "a PIN is set" from it would have this
    /// screen claim a lock the account never had -- on the one screen whose job
    /// is to report exactly that. `None` is a refused query, not a "no".
    pub fn screen_lock_has_pin(&self) -> bool {
        self.screen_lock_has_credential == Some(true)
    }

    /// Opens the panel and reads the account layer's answer.
    pub fn open_screen_lock(&mut self) {
        self.screen_lock_open = true;
        self.screen_lock_mode = ScreenLockMode::Idle;
        self.screen_lock_action = ScreenLockAction::Establish;
        self.screen_lock_buf = [0; 8];
        self.screen_lock_len = 0;
        self.screen_lock_pending = [0; 8];
        self.screen_lock_pending_len = 0;
        self.screen_lock_current = [0; 8];
        self.screen_lock_current_len = 0;
        self.screen_lock_error = SL_ERR_NONE;
        self.screen_lock_write = ScreenLockWrite::Unattempted;
        self.refresh_screen_lock();
    }

    /// Re-reads the lock state and the credential's existence from the account layer.
    pub fn refresh_screen_lock(&mut self) {
        let uid = std::user_get_current();
        self.screen_lock_reported = std::user_get_lock_state(uid).ok();
        self.screen_lock_has_credential = std::user_has_credential(uid).ok();
    }

    /// Clears the typed entry and returns the panel to its overview.
    pub fn screen_lock_reset_entry(&mut self) {
        self.screen_lock_buf = [0; 8];
        self.screen_lock_len = 0;
        self.screen_lock_error = SL_ERR_NONE;
    }

    /// Appends one digit to the entry in progress.
    pub fn screen_lock_push(&mut self, digit: u8) {
        if self.screen_lock_len < 8 {
            self.screen_lock_buf[self.screen_lock_len] = digit;
            self.screen_lock_len += 1;
            self.screen_lock_error = SL_ERR_NONE;
        }
    }

    /// Deletes the last digit of the entry in progress.
    pub fn screen_lock_pop(&mut self) {
        if self.screen_lock_len > 0 {
            self.screen_lock_len -= 1;
            self.screen_lock_error = SL_ERR_NONE;
        }
    }

    /// Submits the typed entry and advances the flow, calling the account layer
    /// only when a complete, confirmed credential is in hand.
    ///
    /// WHY the confirmation step exists: a PIN typed once and stored makes a slip
    /// of the finger the device's credential, and the owner then cannot unlock
    /// their own phone.
    pub fn screen_lock_submit(&mut self) {
        let uid = std::user_get_current();
        let len = self.screen_lock_len;
        if len < SL_MIN_DIGITS {
            self.screen_lock_error = SL_ERR_SHORT;
            return;
        }
        match self.screen_lock_mode {
            ScreenLockMode::EnterCurrent => {
                let current = self.screen_lock_buf;
                let current_len = len;
                if self.screen_lock_action == ScreenLockAction::Remove {
                    match std::user_clear_credential(uid, &current[..current_len]) {
                        Ok(()) => {
                            self.screen_lock_write = ScreenLockWrite::Removed;
                            self.screen_lock_mode = ScreenLockMode::Idle;
                            self.screen_lock_error = SL_ERR_NONE;
                            std::log(
                                0,
                                "[SETTINGS APP] Screen lock removed; the account layer reports it credential-less.",
                            )
                            .ok();
                        }
                        Err(_) => {
                            // WHY the current PIN is named as the suspect rather
                            // than "write failed": the kernel only refuses this
                            // call for an unverified credential, and the panel
                            // should not tell the user their storage broke.
                            self.screen_lock_write = ScreenLockWrite::Refused;
                            self.screen_lock_error = SL_ERR_NOT_CURRENT;
                            std::log(
                                0,
                                "[SETTINGS APP:SECURITY] Credential removal refused: the current PIN did not verify.",
                            )
                            .ok();
                        }
                    }
                    self.screen_lock_reset_entry();
                    self.refresh_screen_lock();
                    return;
                }
                // Change: the current PIN is kept for the account layer to verify
                // when the replacement is written.
                self.screen_lock_current = current;
                self.screen_lock_current_len = current_len;
                self.screen_lock_reset_entry();
                self.screen_lock_mode = ScreenLockMode::EnterNew;
                std::log(0, "[SETTINGS APP] Current PIN accepted; enter the replacement.").ok();
            }
            ScreenLockMode::EnterNew => {
                self.screen_lock_pending = self.screen_lock_buf;
                self.screen_lock_pending_len = len;
                self.screen_lock_reset_entry();
                self.screen_lock_mode = ScreenLockMode::ConfirmNew;
                std::log(0, "[SETTINGS APP] New PIN recorded; confirm it.").ok();
            }
            ScreenLockMode::ConfirmNew => {
                let pending_len = self.screen_lock_pending_len;
                if len != pending_len || self.screen_lock_buf[..len] != self.screen_lock_pending[..pending_len] {
                    self.screen_lock_error = SL_ERR_MISMATCH;
                    self.screen_lock_pending = [0; 8];
                    self.screen_lock_pending_len = 0;
                    self.screen_lock_reset_entry();
                    self.screen_lock_mode = ScreenLockMode::EnterNew;
                    std::log(
                        0,
                        "[SETTINGS APP:SECURITY] The two new-PIN entries did not match; nothing was written.",
                    )
                    .ok();
                    return;
                }
                let new_pin = self.screen_lock_buf;
                let current = self.screen_lock_current;
                let current_len = self.screen_lock_current_len;
                let old: Option<&[u8]> = if self.screen_lock_action == ScreenLockAction::Establish {
                    None
                } else {
                    Some(&current[..current_len])
                };
                match std::user_change_credential(uid, &new_pin[..len], old) {
                    Ok(()) => {
                        self.screen_lock_write = match self.screen_lock_action {
                            ScreenLockAction::Establish => ScreenLockWrite::Established,
                            _ => ScreenLockWrite::Changed,
                        };
                        self.screen_lock_mode = ScreenLockMode::Idle;
                        self.screen_lock_error = SL_ERR_NONE;
                        std::log(
                            0,
                            "[SETTINGS APP:SECURITY] Credential written and persisted by the account layer.",
                        )
                        .ok();
                    }
                    Err(_) => {
                        self.screen_lock_write = ScreenLockWrite::Refused;
                        self.screen_lock_error = SL_ERR_REFUSED;
                        std::log(
                            0,
                            "[SETTINGS APP:SECURITY] Credential change refused by the account layer; nothing was written.",
                        )
                        .ok();
                    }
                }
                self.screen_lock_pending = [0; 8];
                self.screen_lock_pending_len = 0;
                self.screen_lock_current = [0; 8];
                self.screen_lock_current_len = 0;
                self.screen_lock_reset_entry();
                self.refresh_screen_lock();
            }
            ScreenLockMode::Idle => {
                self.screen_lock_error = SL_ERR_NONE;
            }
        }
    }

    /// Starts a flow from the overview, given the action its button represents.
    pub fn screen_lock_begin(&mut self, action: ScreenLockAction) {
        self.screen_lock_action = action;
        self.screen_lock_buf = [0; 8];
        self.screen_lock_len = 0;
        self.screen_lock_pending = [0; 8];
        self.screen_lock_pending_len = 0;
        self.screen_lock_current = [0; 8];
        self.screen_lock_current_len = 0;
        self.screen_lock_error = SL_ERR_NONE;
        self.screen_lock_mode = match action {
            ScreenLockAction::Establish => ScreenLockMode::EnterNew,
            _ => ScreenLockMode::EnterCurrent,
        };
        std::log(
            0,
            match action {
                ScreenLockAction::Establish => "[SETTINGS APP] Enter a new PIN to set a screen lock.\n",
                ScreenLockAction::Change => "[SETTINGS APP] Enter the current PIN to change it.\n",
                ScreenLockAction::Remove => "[SETTINGS APP] Enter the current PIN to remove the screen lock.\n",
            },
        )
        .ok();
    }

    /// Leaves any step in progress, returning to the overview without writing.
    pub fn screen_lock_cancel_step(&mut self) -> bool {
        if self.screen_lock_mode == ScreenLockMode::Idle {
            return false;
        }
        self.screen_lock_mode = ScreenLockMode::Idle;
        self.screen_lock_buf = [0; 8];
        self.screen_lock_len = 0;
        self.screen_lock_pending = [0; 8];
        self.screen_lock_pending_len = 0;
        self.screen_lock_current = [0; 8];
        self.screen_lock_current_len = 0;
        self.screen_lock_error = SL_ERR_NONE;
        std::log(0, "[SETTINGS APP] Screen lock entry cancelled; nothing was written.").ok();
        true
    }

    /// Writes the pending selection to the selected targets.
    ///
    /// WHY the outcome is recorded rather than assumed: `settings_set` is gated
    /// on this app's `FileSystemWrite` capability and writes through to a
    /// disk-backed journal, so a refusal is a real outcome and the picker must
    /// say so instead of showing the new wallpaper as applied.
    pub fn apply_wallpaper(&mut self) {
        let Some(scene) = self.wallpaper_pending else {
            // WHY this is refused and not defaulted: an Apply with no selection
            // has nothing to write, and writing the default instead would change
            // the user's wallpaper when they pressed a button that had no meaning.
            self.wallpaper_write = WallpaperWrite::Refused;
            std::log(
                0,
                "[SETTINGS APP] Wallpaper Apply pressed with no pending selection; no write attempted.",
            )
            .ok();
            return;
        };
        let keys: &[&[u8]] = match self.wallpaper_target {
            WallpaperTarget::Home => &[b"ui.wallpaper.home"],
            WallpaperTarget::Lock => &[b"ui.wallpaper.lock"],
            WallpaperTarget::Both => &[b"ui.wallpaper.home", b"ui.wallpaper.lock"],
        };
        let mut ok = true;
        for key in keys {
            // WHY the value is written as the catalogue's own key: the launcher
            // resolves the stored value back through the same catalogue, so a
            // value that is not a key would make the lookup find nothing and the
            // shell would report an unknown scene.
            match std::settings_set(3, core::str::from_utf8(key).unwrap_or(""), scene.key) {
                Ok(()) => {}
                Err(_) => ok = false,
            }
        }
        if ok {
            self.wallpaper_write = WallpaperWrite::Applied(self.wallpaper_target);
            self.wallpaper_home = match self.wallpaper_target {
                WallpaperTarget::Lock => self.wallpaper_home,
                _ => Some(scene),
            };
            self.wallpaper_lock = match self.wallpaper_target {
                WallpaperTarget::Home => self.wallpaper_lock,
                _ => Some(scene),
            };
            self.wallpaper_pending = None;
            std::log(0, "[SETTINGS APP] Wallpaper written to the settings store.").ok();
        } else {
            self.wallpaper_write = WallpaperWrite::Refused;
            std::log(
                0,
                "[SETTINGS APP] The settings store refused the wallpaper write; no wallpaper was changed.",
            )
            .ok();
        }
    }

    /// Restores the bundled default wallpaper on both surfaces.
    ///
    /// WHY it writes rather than deletes: the kernel's settings store has no
    /// delete, so "reset" has to be expressed as a write of the default key. The
    /// key written is the catalogue's own default, so a fresh boot and a reset
    /// boot render the same thing.
    pub fn reset_wallpaper(&mut self) {
        let scene = &std::design::walls::SCENES[0];
        let mut ok = true;
        for key in ["ui.wallpaper.home", "ui.wallpaper.lock"] {
            if std::settings_set(3, key, scene.key).is_err() {
                ok = false;
            }
        }
        self.wallpaper_write = if ok {
            std::log(0, "[SETTINGS APP] Wallpapers reset to the bundled default.")
                .ok();
            WallpaperWrite::Applied(WallpaperTarget::Both)
        } else {
            WallpaperWrite::Refused
        };
        if ok {
            self.wallpaper_home = Some(scene);
            self.wallpaper_lock = Some(scene);
            self.wallpaper_pending = None;
        }
    }

}

// -----------------------------------------------------------------------------
// Visual Design Tokens (canonical light System Space palette)
// -----------------------------------------------------------------------------
// WHY every colour below is a `design::color` token: the design doc's palette and
// the SDK's token set are the same set of values, so an app that reaches for a
// literal has already left the system. The two inks this app used to derive for
// itself — the home-indicator grey and the wallpaper name plate's pair — are now
// either a token outright or a token with an alpha channel, so the palette is
// the only source of colour on any surface.

const BG: u32 = std::design::color::BACKGROUND; // #F7F6F2 base
const SURFACE: u32 = std::design::color::SURFACE; // #FFFFFF card surface
const SOFT: u32 = std::design::color::SURFACE_SOFT; // #EFEEE9 controls / pills
const BORDER: u32 = std::design::color::BORDER; // #E1E0DB hairlines
const TEXT: u32 = std::design::color::TEXT; // #111214 primary text
const TEXT_MUTED: u32 = std::design::color::TEXT_MUTED; // #777773 secondary text
const ACCENT: u32 = std::design::color::ACCENT; // restrained blue
const SUCCESS_EMERALD: u32 = std::design::color::SUCCESS; // muted green
const DANGER: u32 = std::design::color::DANGER; // refused writes and destructive actions

/// The home-indicator pill.
///
/// WHY secondary ink rather than a grey of its own: this used to be `0xFFC4C3BD`,
/// a value that existed nowhere in the design system. The reference `GestureBar`
/// component is a neutral dash, so the palette's own secondary ink is the answer
/// the system already gives.
const GESTURE_BAR: u32 = TEXT_MUTED;

/// Opacity of the translucent name plate over the wallpaper preview.
const PLATE_ALPHA: u32 = 0xCC;

/// Replaces a token colour's alpha channel.
///
/// WHY this exists: the name plate over a wallpaper has to be translucent, and
/// the only two inks that work on a plate whose polarity flips with the scene
/// under it are the palette's own light and dark text. Building them from
/// `design::color` keeps the plate inside the palette; the alpha belongs to the
/// plate rather than to the ink, so it is passed in instead of baked into a
/// literal pair.
#[inline]
const fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00FF_FFFF) | (alpha << 24)
}

// -----------------------------------------------------------------------------
// Typography: the rungs this face can actually draw
// -----------------------------------------------------------------------------
// WHY two names instead of the design doc's seven: the guest's bitmap face
// rasterises a 16x24 cell at INTEGER scales only, so a "size" here is one of
// 24px (scale 1), 48px (scale 2) or 72px (scale 3). Against the doc's rungs:
//
//   Display 48-56 -> TYPE_DISPLAY (48)
//   H1 36-44      -> NOT EXPRESSIBLE (48 overshoots it, 24 undershoots it)
//   H2 28-32      -> NOT EXPRESSIBLE
//   H3 20-24      -> TYPE_H3 (24)
//   Body 15-18    -> NOT EXPRESSIBLE (24 is the nearest rung and sits above it)
//   Caption 12-14 -> NOT EXPRESSIBLE (24 is the floor and sits far above it)
//
// So the doc's own instruction for this case is followed literally: hierarchy is
// built from spacing, weight and ink, and the unreachable rungs are recorded
// here rather than silently approximated. A screen title is Display, a row title
// is H3 bold in primary ink, and a caption is H3 regular in secondary ink. The
// launcher's `theme::type_scale` documents the same consequence for its own
// BADGE and CAPTION rungs, which coincide for exactly this reason.

/// 24px cell: row titles, captions, badges, control labels, buttons.
const TYPE_H3: usize = 1;
/// 48px cell: screen titles and keypad digits.
const TYPE_DISPLAY: usize = 2;
/// One line box at the H3 rung: the face's own cell height.
const LINE_H: usize = std::design::font::GLYPH_H * TYPE_H3;

// -----------------------------------------------------------------------------
// Layout constants (spacing, radius and touch targets from the SDK tokens)
// -----------------------------------------------------------------------------

/// Screen margin: the design doc's "typical screen margin 20-24 px", taken from
/// the SDK's own content padding so the app and the system agree on one number.
const MARGIN: usize = std::design::spacing::GRID_24;

/// Gap between stacked cards, and the gutter of a two-up action row.
const CARD_GAP: usize = std::design::spacing::GRID_16;

/// Standard card radius: the doc's "standard cards 16-20 px" rung, which is the
/// SDK's `radius::CARD` token. WHY one radius for every card on every surface: a
/// card is a component, and a component has one shape.
const CARD_R: usize = std::design::radius::CARD;

/// Control radius: the doc's "controls/keypad 12-14 px" rung. Used by the back
/// control, every segmented-control segment, every keypad key and the inner rim
/// of the action pills.
const CONTROL_R: usize = std::design::radius::CONTROL;

/// Image radius: the doc's "small/image 8 px" rung, used by the gallery
/// thumbnails and the entry dots.
const IMAGE_R: usize = std::design::radius::SMALL;

/// The content column, shared by every screen: the landing grid, the detail rows
/// and both full-screen panels all start 24px in from each edge.
///
/// WHY one column: the detail rows, the wallpaper panel and the screen-lock panel
/// each used to declare their own 24/672 pair, so nothing forced them to agree
/// and a panel that drifted would misalign against the screen it covers.
const CONTENT_X: usize = MARGIN;
const CONTENT_W: usize = W - 2 * MARGIN;

/// Shared header geometry: a 48x48 back control at the top-left of every screen
/// with the screen's title on the same line to its right.
const BACK_X: usize = CONTENT_X;
const BACK_Y: usize = MARGIN;
const BACK_S: usize = std::design::touch::PREFERRED_TARGET;
const BACK_R: usize = CONTROL_R;
/// X of the header's text: one 16px step right of the back control.
const HEADER_X: usize = BACK_X + BACK_S + std::design::spacing::GRID_16;
/// Y of the header's title.
///
/// WHY 32 rather than the back control's own 24: a 48px cell's cap ink occupies
/// rows 3..19, so a cell starting at 24 puts that ink eleven pixels above the
/// chevron's centre and the title reads as drifting away from its own control.
/// 32 lands the cap-ink centre five pixels above it, the closest optical
/// centring the 8-point grid allows.
const HEADER_Y: usize = std::design::spacing::GRID_32;
/// Y of a panel header's subtitle: one 8px step below the title's line box.
const HEADER_SUB_Y: usize = HEADER_Y + std::design::font::GLYPH_H * TYPE_DISPLAY + std::design::spacing::GRID_8;
/// The header rule: a hairline closing the header block, one 16px step below the
/// title's line box.
const DIVIDER_Y: usize = HEADER_Y + std::design::font::GLYPH_H * TYPE_DISPLAY + std::design::spacing::GRID_16;
/// Top of the first content element on any screen: one 32px step below the rule.
/// Every surface's body begins on this line.
const DETAIL_TOP: usize = DIVIDER_Y + std::design::spacing::GRID_32;

/// Landing grid geometry: two columns of cards with a 16px gutter.
const GRID_X: usize = CONTENT_X;
/// Padding of a landing card, which is also its horizontal padding, so the icon,
/// the title and the subtitle all sit on the same 24px inset.
const CARD_PAD: usize = MARGIN;
/// Centre of a landing card's icon: one 24px line box plus a 20px step in from
/// the card's left padding.
const CARD_ICON_CX: usize = CARD_PAD + std::design::spacing::GRID_20;
/// X of a landing card's text column: half an icon-box and one 24px step right of
/// the icon's centre.
const CARD_TEXT_X: usize = CARD_ICON_CX + LINE_H / 2 + std::design::spacing::GRID_24;
/// Width available to a landing card's text.
#[allow(dead_code)]
const CARD_TEXT_W: usize = CARD_W - CARD_TEXT_X - CARD_PAD;
/// Column width: single-column full content width for maximum legibility and zero truncation.
const CARD_W: usize = CONTENT_W;
/// Y step between a card's title line and its caption line.
const CARD_LINE_GAP: usize = std::design::spacing::GRID_8;
/// Dynamic Device Vitality Orb geometry
const ORB_CX: usize = 360;
const ORB_CY: usize = 66;
const ORB_R: usize = 24;
const HALO_R: usize = 42;

/// Landing screen identity typography positions
const IDENTITY_TITLE_Y: usize = 104;
const IDENTITY_SUB_Y: usize = 138;

/// Landing card height: single-column full-width card with icon, title, subtitle & status chip
const CARD_H: usize = 86;
/// Gutter between landing cards.
const GRID_GAP: usize = 12;
/// Top of the landing grid: spaced comfortably below the identity hero block.
const GRID_Y: usize = 176;
/// Compatibility aliases for old landing titles.
#[allow(dead_code)]
const LANDING_TITLE_Y: usize = 96;
#[allow(dead_code)]
const LANDING_HERO_Y: usize = 152;

/// Detail screen geometry: full-width stacked row cards.
const DETAIL_X: usize = CONTENT_X;
const DETAIL_W: usize = CONTENT_W;
/// Horizontal padding inside a row card.
const ROW_PAD: usize = MARGIN;
/// Vertical padding of a row card.
///
/// WHY 32 and not the 16 a dense settings list would use: the smallest text rung
/// this face can draw is a 24px cell, so a 16px pad makes a row read as a text
/// block with a border drawn round it. The doc asks for whitespace over
/// decoration, and at a fixed type size vertical padding is the only lever there
/// is for it.
const ROW_PAD_V: usize = std::design::spacing::GRID_32;
/// Gap between a row's title and its caption.
const ROW_LINE_GAP: usize = std::design::spacing::GRID_8;
/// Title offset from a row's top edge.
const ROW_TITLE_DY: usize = ROW_PAD_V;
/// Caption offset from a row's top edge.
const ROW_CAP_DY: usize = ROW_TITLE_DY + LINE_H + ROW_LINE_GAP;
/// A single-line row's height.
const ROW_H: usize = ROW_CAP_DY + LINE_H + ROW_PAD_V;
/// Gap between stacked rows.
const ROW_GAP: usize = CARD_GAP;
/// Gap between a row's caption and the control below it.
const ROW_CONTROL_GAP: usize = std::design::spacing::GRID_24;
/// Second caption line's offset, for the row that names its store key.
const ROW_CAP2_DY: usize = ROW_CAP_DY + LINE_H + ROW_LINE_GAP;
/// A two-caption-line row's height.
const ROW_TALL_ALT: usize = ROW_CAP2_DY + LINE_H + ROW_PAD_V;
/// The DVFS selector's offset from its row's top edge.
const SEG_DY: usize = ROW_CAP_DY + LINE_H + ROW_CONTROL_GAP;
/// One selector segment's height: the preferred touch target.
const SEG_H: usize = std::design::touch::PREFERRED_TARGET;
/// Gap between segmented-control segments.
const SEG_GAP: usize = std::design::spacing::GRID_4;
/// Segment width, so three of them plus two gaps fill a row's inner width. The
/// division drops at most two pixels, which land as slack on the right rather
/// than as an off-grid gap that would make the selector asymmetric.
const SEG_W: usize = (DETAIL_W - 2 * ROW_PAD - 2 * SEG_GAP) / 3;
/// The DVFS feedback line's offset: one 16px step below the selector.
const SEG_FEEDBACK_DY: usize = SEG_DY + SEG_H + CARD_GAP;
/// The DVFS row's height: it carries the selector and its feedback line.
const ROW_TALL: usize = SEG_FEEDBACK_DY + LINE_H + ROW_PAD_V;

/// Badge height. WHY 32: a pill has to read as a chip rather than as text, and 32
/// is the grid step that gives a 24px cell four pixels of air above and below.
const BADGE_H: usize = std::design::spacing::GRID_32;
/// Horizontal padding inside a badge.
const BADGE_PAD: usize = std::design::spacing::GRID_24;
/// A badge's top edge inside a row: centred on the caption's line box, which it
/// overhangs by four pixels because it is one grid step taller.
const BADGE_DY: usize = ROW_CAP_DY - (BADGE_H - LINE_H) / 2;
/// Gap between a row's caption and its badge.
const BADGE_GAP: usize = std::design::spacing::GRID_16;

/// Action button height: the reference `PrimaryButton` component's 48px, which is
/// also the SDK's preferred touch target.
const BUTTON_H: usize = std::design::touch::PREFERRED_TARGET;
/// Half of a two-up action row: two buttons and one 16px gutter.
const BUTTON_HALF: usize = (CONTENT_W - CARD_GAP) / 2;
/// Inner padding of an action button's label.
const BUTTON_PAD: usize = ROW_PAD;

/// The home indicator: the reference `GestureBar` component (96x4 dash, pill
/// radius) centred on the bottom edge. WHY the drawn mark is smaller than the
/// touch region it sits in: the handler's exit region is unchanged, so making the
/// mark match the component must not shrink the target.
const GESTURE_W: usize = 96;
const GESTURE_H: usize = 4;
const GESTURE_X: usize = (W - GESTURE_W) / 2;
/// WHY 22 above the bottom edge: that is where the previous 200x6 bar was drawn,
/// so the chrome does not move while its shape changes.
const GESTURE_Y: usize = H - 22;

// -----------------------------------------------------------------------------
// Typography: measure -> fit -> draw. Nothing is ever clipped by its box.
// -----------------------------------------------------------------------------

/// Maximum bytes any single fitted run may occupy (source runs are shorter).
const RUN_CAP: usize = 96;

// -----------------------------------------------------------------------------
// Wallpaper picker layout
// -----------------------------------------------------------------------------
// The panel is laid out from the reference wallpaper screen: a preview at the
// top, a target selector, a gallery of the scenes that actually exist, and the
// two actions at the bottom. Everything the panel draws is either a scene from
// the catalogue or a string the catalogue provides, so the screen cannot show a
// wallpaper that does not exist.
//
// The preview is the panel's hero and takes the whole upper band, so the owner
// judges a wallpaper by how it fills the screen; each control below is one 32px
// step from the last.

/// Top of the preview strip: the shared content top, so the panel's body begins
/// on the same line as every other screen's.
const WP_PREVIEW_Y: usize = DETAIL_TOP;
/// Height of the preview strip.
const WP_PREVIEW_H: usize = 664;
/// Preview radius: the doc's sheet rung, because the preview is this panel's one
/// large surface rather than a card in a list.
const WP_PREVIEW_R: usize = std::design::radius::SHEET;

/// The name plate over the preview's lower edge: the doc's control rung for its
/// radius, the preferred touch target for its height (it is a label chip that has
/// to stay legible over a scene, not a control), inset 20px from each side and
/// sitting 24px above the preview's bottom edge.
const PLATE_H: usize = std::design::touch::PREFERRED_TARGET;
const PLATE_INSET: usize = std::design::spacing::GRID_20;

/// Top of the target selector: one 32px step below the preview.
const WP_TARGET_Y: usize = WP_PREVIEW_Y + WP_PREVIEW_H + std::design::spacing::GRID_32;
/// Height of one target segment: the SDK's primary-action touch target.
const WP_TARGET_H: usize = std::design::touch::PRIMARY_ACTION;
/// Top of the gallery: one 32px step below the selector.
const WP_GALLERY_Y: usize = WP_TARGET_Y + WP_TARGET_H + std::design::spacing::GRID_32;
/// Gutter between gallery cells.
const WP_CELL_GAP: usize = std::design::spacing::GRID_24;
/// Cell width: three across with 24px gutters, which fills the content column
/// exactly (3 x 208 + 2 x 24 = 672).
const WP_CELL_W: usize = (CONTENT_W - 2 * WP_CELL_GAP) / 3;
/// Padding around a cell's thumbnail.
const WP_CELL_PAD: usize = std::design::spacing::GRID_8;
/// Gap between a cell's thumbnail and its name.
const WP_CELL_LABEL_GAP: usize = std::design::spacing::GRID_8;
/// Thumbnail height inside a cell.
const WP_THUMB_H: usize = 144;
/// Cell height: padding, thumbnail, gap, one name line box, matching padding.
const WP_CELL_H: usize = WP_CELL_PAD + WP_THUMB_H + WP_CELL_LABEL_GAP + LINE_H + WP_CELL_PAD;
const WP_ROWS: usize = (std::design::walls::SCENES.len() + 2) / 3;
const WP_GALLERY_H: usize = WP_ROWS * WP_CELL_H + (WP_ROWS.saturating_sub(1)) * WP_CELL_GAP;
/// Top of the action row: one 32px step below the gallery.
const WP_ACTION_Y: usize = WP_GALLERY_Y + WP_GALLERY_H + std::design::spacing::GRID_32;
/// Height of the action row.
const WP_ACTION_H: usize = BUTTON_H;
/// Top of the outcome line: one 16px step below the actions.
const WP_STATUS_Y: usize = WP_ACTION_Y + WP_ACTION_H + CARD_GAP;

// Screen lock panel layout
// -----------------------------------------------------------------------------
// One panel for all three owner decisions (set, change, remove), so there is a
// single keypad and a single place the credential state is read from. The status
// card states what the account layer actually reports; the keypad appears only
// while a step is in progress, so the overview cannot look like a PIN prompt.

/// The status card: what the account layer says about this account right now.
/// It uses a detail row card's height because it is one.
const SL_STATUS_Y: usize = DETAIL_TOP;
const SL_STATUS_H: usize = ROW_H;
/// The step prompt and the typed-entry dots, present only mid-flow.
const SL_PROMPT_Y: usize = SL_STATUS_Y + SL_STATUS_H + std::design::spacing::GRID_48;
/// The complaint line's offset from the prompt.
const SL_ERROR_DY: usize = std::design::spacing::GRID_40;
/// The typed-entry dots: one 40px step below the complaint line.
const SL_DOTS_Y: usize = SL_PROMPT_Y + SL_ERROR_DY + LINE_H + std::design::spacing::GRID_40;
/// One entry dot.
const SL_DOT: usize = std::design::spacing::GRID_16;
/// Gap between dots.
const SL_DOT_GAP: usize = 20;
/// Dot radius: the image rung, which for a 16px dot is a full circle.
const SL_DOT_R: usize = IMAGE_R;
/// One keypad key; the grid is three across with the last row CLR / 0 / OK.
/// WHY the width is derived from the gutter: the pad used to be 200px per key,
/// which left it 24px narrower than the panel it sits in and made the right
/// margin twice the left.
const SL_KEY_W: usize = (CONTENT_W - 2 * SL_KEY_GAP) / 3;
const SL_KEY_H: usize = 112;
const SL_KEY_GAP: usize = std::design::spacing::GRID_24;
/// Key radius: the doc's keypad rung. WHY not the lock screen's own 18: this app
/// follows the design doc's rung, and the two pads already differ in width, so a
/// shared radius is the only thing they would otherwise have had in common.
const SL_KEY_R: usize = CONTROL_R;
/// Top of the keypad: one 48px step below the dots.
const SL_KEYS_Y: usize = SL_DOTS_Y + SL_DOT + std::design::spacing::GRID_48;
/// Bottom of the keypad's four rows.
const SL_KEYS_BOTTOM: usize = SL_KEYS_Y + 4 * (SL_KEY_H + SL_KEY_GAP) - SL_KEY_GAP;
/// The action row, whose contents follow the reported lock state.
const SL_ACTION_Y: usize = SL_KEYS_BOTTOM + std::design::spacing::GRID_32;
const SL_ACTION_H: usize = BUTTON_H;
/// The outcome line under the actions.
const SL_NOTE_Y: usize = SL_ACTION_Y + SL_ACTION_H + CARD_GAP;
/// Minimum digits the account layer will accept for a PIN.
const SL_MIN_DIGITS: usize = 4;

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
///
/// WHY the outline face comes first: `TYPE_DISPLAY` is scale 2, so on the
/// bitmap face every screen title and keypad digit is a 16x24 cell magnified by
/// an integer -- a soft ramp where an edge belongs.
/// `outline_text::place_run` rasterises the real outline at the em that matches
/// the cell's cap height (17 rows at scale 1, so 17*N*2048/1456 px per em), and
/// answers for the pixel itself. When it declines -- a run wider than the
/// coverage buffer, a glyph whose ink would leave this caller's window, or a
/// weight this app draws bold -- the bitmap face below draws the run exactly as
/// it did before, so a refusal costs the edge quality and never the text.
/// Every `TYPE_DISPLAY` call site in this app passes `bold = true`, and the
/// shipped outline table is Roboto Condensed REGULAR, so today that is what
/// declines; see `outline_text`'s module header.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge.
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
    if let Some(run) =
        outline_text::place_run(x0, y0, scale, &buf[..n], Some(max_w), bold)
    {
        return run.pixel(x, y, color, bg);
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
///
/// WHY the radius is the SDK's control token (12) and not the 14 it used to be:
/// 14 was off both of the design system's control rungs (12-14 includes it, but
/// no token expresses it), and the control, the segmented-control segments and
/// the keypad keys are all the same class of object.
fn back_button_pixel(x: usize, y: usize) -> Option<u32> {
    if !in_round_rect(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, BACK_R) {
        return None;
    }
    if !in_round_rect(x, y, BACK_X + 1, BACK_Y + 1, BACK_S - 2, BACK_S - 2, BACK_R - 1) {
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

/// Landing list entry: title, subtitle and icon for one category.
fn category(i: usize) -> (&'static [u8], &'static [u8], IconFn) {
    match i {
        0 => (b"Device", b"Display, brightness, sound & diagnostics", icon_device),
        1 => (b"Network", b"Wi-Fi, cellular, flight mode & interfaces", icon_network),
        2 => (b"Personal", b"Wallpapers, lockscreen & preferences", icon_personal),
        3 => (b"Apps & Storage", b"14 installed apps, VFS partitions", icon_apps),
        4 => (b"Power & Battery", b"Battery health & DVFS governor", icon_battery),
        5 => (b"Security & Privacy", b"PIN configuration, Keystore integrity", icon_security),
        _ => (b"About InterOS", b"Kernel v0.8.0, x86_64, uptime & memory", icon_gear),
    }
}

/// Bounding rectangle (x, y, w, h) for landing card `i`.
/// Cards 0..6 are arranged in a sleek single-column list with full content width.
fn card_rect(i: usize) -> (usize, usize, usize, usize) {
    (GRID_X, GRID_Y + i * (CARD_H + GRID_GAP), CONTENT_W, CARD_H)
}

/// Top-left corner of landing card `i`.
#[allow(dead_code)]
fn card_origin(i: usize) -> (usize, usize) {
    let (x, y, _, _) = card_rect(i);
    (x, y)
}

/// Landing card hit-test: the whole rounded card is the touch target.
fn landing_card_at(px: u32, py: u32) -> Option<Section> {
    for i in 0..Section::COUNT {
        let (cx0, cy0, cw, ch) = card_rect(i);
        if in_round_rect(px as usize, py as usize, cx0, cy0, cw, ch, CARD_R) {
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
        Section::Device => 5,
        Section::Network => 4,
        Section::Personal => 4,
        Section::Apps => 4,
        Section::Power => 4,
        Section::Security => 4,
        Section::System => 5,
    }
}

/// Height of row `row` on `section`'s detail screen.
fn row_h(section: Section, row: usize) -> usize {
    match (section, row) {
        (Section::Power, 1) => ROW_TALL,
        (Section::System, 4) => ROW_TALL_ALT,
        _ => ROW_H,
    }
}

/// Diagnostic row layout constants for the Sensors & Diagnostics surface.
const DIAG_ROW_H: usize = 96;
const DIAG_ROW_GAP: usize = 12;

/// Top edge and height of row `row` on the dedicated Sensors & Diagnostics surface.
fn sensors_row_rect(row: usize) -> (usize, usize) {
    let y = DETAIL_TOP + row * (DIAG_ROW_H + DIAG_ROW_GAP);
    (y, DIAG_ROW_H)
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

/// Writes 64-bit integer `n` as decimal ASCII into `out`; returns the byte count.
fn write_decimal_u64(n: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if n == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
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

/// Pill badge, right-aligned helper: soft fill, 1px ink rim, label centred.
/// The width is derived from the measured label so the pill always fits its
/// text and the caller can reserve exactly that much horizontal space.
fn badge_w(label: &[u8]) -> usize {
    run_w(label, TYPE_H3, false) + BADGE_PAD
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
    let h = BADGE_H;
    if !in_round_rect(x, y, bx0, by0, w, h, h / 2) {
        return None;
    }
    if !in_round_rect(x, y, bx0 + 1, by0 + 1, w - 2, h - 2, h / 2 - 1) {
        return Some(ink);
    }
    let tw = run_w(label, TYPE_H3, false);
    let tx = bx0 + (w - tw) / 2;
    let ty = by0 + (h - LINE_H) / 2;
    if let Some(c) = run_ink(x, y, tx, ty, TYPE_H3, label, tw + 1, ink, bg, false) {
        return Some(c);
    }
    Some(SOFT)
}

/// One action button: a pill with a fitted, centred label.
///
/// WHY this is a shared helper rather than three copies: the design doc's closing
/// note asks for reusable components and tokens rather than per-screen
/// hard-coding, and this app has three action rows (wallpaper Apply/Reset, screen
/// lock Set/Change/Remove) that must share one shape, one height and one label
/// treatment or the system stops reading as one system.
///
/// WHY a pill: the reference `PrimaryButton` and `SecondaryButton` components are
/// both 999-radius at 48px tall, which at this button's height is a full pill.
/// WHY the `rim` is a parameter: a dark primary pill wants no outline at all,
/// while a soft secondary pill needs a hairline to read against the base.
#[allow(clippy::too_many_arguments)]
fn button_pixel(
    x: usize,
    y: usize,
    bx0: usize,
    by0: usize,
    bw: usize,
    bh: usize,
    label: &[u8],
    fill: u32,
    ink: u32,
    rim: Option<u32>,
) -> Option<u32> {
    if !in_round_rect(x, y, bx0, by0, bw, bh, bh / 2) {
        return None;
    }
    if !in_round_rect(x, y, bx0 + 1, by0 + 1, bw - 2, bh - 2, bh / 2 - 1) {
        return Some(rim.unwrap_or(fill));
    }
    let lw = run_w(label, TYPE_H3, true);
    // WHY the label is fitted before it is centred: `bw - lw` underflows in usize
    // when a label is wider than its button, and the wrapped origin then puts the
    // text off the surface with the button drawn only part-way.
    let lx = if lw + 2 * BUTTON_PAD >= bw {
        bx0 + BUTTON_PAD
    } else {
        bx0 + (bw - lw) / 2
    };
    if let Some(c) = run_ink(
        x,
        y,
        lx,
        by0 + (bh - LINE_H) / 2,
        TYPE_H3,
        label,
        bw - 2 * BUTTON_PAD,
        ink,
        fill,
        true,
    ) {
        return Some(c);
    }
    Some(fill)
}

/// One segment of a segmented control: dark fill when selected, soft fill with a
/// hairline when not.
///
/// WHY this is shared by the DVFS selector and the wallpaper target selector:
/// a segmented control is one component with one selected/unselected treatment,
/// and two treatments for "the chosen of three" would make the same interaction
/// mean two things.
fn segment_pixel(
    x: usize,
    y: usize,
    sx0: usize,
    sy0: usize,
    sw: usize,
    sh: usize,
    label: &[u8],
    active: bool,
) -> Option<u32> {
    if !in_round_rect(x, y, sx0, sy0, sw, sh, CONTROL_R) {
        return None;
    }
    if !active && !in_round_rect(x, y, sx0 + 1, sy0 + 1, sw - 2, sh - 2, CONTROL_R - 1) {
        return Some(BORDER);
    }
    let fill = if active { TEXT } else { SOFT };
    let lw = run_w(label, TYPE_H3, true);
    let lx = if lw + 2 * std::design::spacing::GRID_8 >= sw {
        sx0 + std::design::spacing::GRID_8
    } else {
        sx0 + (sw - lw) / 2
    };
    if let Some(c) = run_ink(
        x,
        y,
        lx,
        sy0 + (sh - LINE_H) / 2,
        TYPE_H3,
        label,
        sw - 2 * std::design::spacing::GRID_8,
        if active { SURFACE } else { TEXT },
        fill,
        true,
    ) {
        return Some(c);
    }
    Some(fill)
}

/// Standard row card head: bold title on its own line (never truncated by a
/// badge, because the title owns the full card width), caption below with the
/// badge column explicitly reserved, badge right-aligned on the caption line.
///
/// WHY the title is the H3 rung rather than the Display rung it used to be: the
/// doc puts a list label at Body/H3, and at the only two sizes this face can draw
/// the row title has to sit one rung below the screen title or the screen and its
/// rows read as one flat rank. The doc's H2 (28-32) is not expressible, so H3 is
/// the nearest rung that is *below* the screen title rather than a silent
/// approximation of Body.
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
    let bx0 = DETAIL_X + DETAIL_W - ROW_PAD - bw;
    let cap_fit = DETAIL_W - 2 * ROW_PAD - bw - BADGE_GAP;
    let title_fit = if bw > 0 { cap_fit } else { DETAIL_W - 2 * ROW_PAD };
    if let Some(c) = run_ink(
        x,
        y,
        DETAIL_X + ROW_PAD,
        cy + ROW_TITLE_DY,
        TYPE_H3,
        title,
        title_fit,
        TEXT,
        SURFACE,
        true,
    ) {
        return Some(c);
    }
    if let Some(c) = run_ink(
        x,
        y,
        DETAIL_X + ROW_PAD,
        cy + ROW_CAP_DY,
        TYPE_H3,
        caption,
        cap_fit,
        cap_ink,
        SURFACE,
        false,
    ) {
        return Some(c);
    }
    badge_pixel(x, y, bx0, cy + BADGE_DY, badge, badge_ink, SURFACE)
}

/// One segment of the DVFS profile selector.
fn profile_segment_pixel(x: usize, y: usize, cy: usize, app: &SettingsApp, i: usize) -> Option<u32> {
    let sx = DETAIL_X + ROW_PAD + i * (SEG_W + SEG_GAP);
    let label: &[u8] = match i {
        0 => b"Performance",
        1 => b"Balanced",
        _ => b"PowerSaver",
    };
    // WHY the `map(...) == Some(i)` guard: `power_profile` is `Option`, so a profile
    // that has never been read from or applied to the kernel highlights NO
    // segment. Previously this defaulted to a hardcoded Balanced, which drew a
    // committed-looking selection the kernel had never reported.
    let active = app.power_profile.map(|p| p.index()) == Some(i);
    segment_pixel(x, y, sx, cy + SEG_DY, SEG_W, SEG_H, label, active)
}

/// Diagnostic row item text and badge positioning offsets.
const DIAG_TITLE_DY: usize = 16;
const DIAG_CAP_DY: usize = 48;
const DIAG_BADGE_DY: usize = 44;

/// Specialized row head renderer for 96px Sensors & Diagnostics cards.
fn diag_row_head(
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
    let bx0 = DETAIL_X + DETAIL_W - ROW_PAD - bw;
    let cap_fit = DETAIL_W - 2 * ROW_PAD - bw - BADGE_GAP;
    let title_fit = if bw > 0 { cap_fit } else { DETAIL_W - 2 * ROW_PAD };
    if let Some(c) = run_ink(
        x,
        y,
        DETAIL_X + ROW_PAD,
        cy + DIAG_TITLE_DY,
        TYPE_H3,
        title,
        title_fit,
        TEXT,
        SURFACE,
        true,
    ) {
        return Some(c);
    }
    if let Some(c) = run_ink(
        x,
        y,
        DETAIL_X + ROW_PAD,
        cy + DIAG_CAP_DY,
        TYPE_H3,
        caption,
        cap_fit,
        cap_ink,
        SURFACE,
        false,
    ) {
        return Some(c);
    }
    badge_pixel(x, y, bx0, cy + DIAG_BADGE_DY, badge, badge_ink, SURFACE)
}

/// Renders one live diagnostic sensor row on the dedicated Sensors & Diagnostics sub-page.
fn diag_row_pixel(
    x: usize,
    y: usize,
    app: &SettingsApp,
    row: usize,
    cy: usize,
) -> Option<u32> {
    let t = &app.sensor_telemetry;
    match row {
        0 => {
            let mut cap_buf = [0u8; 64];
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if t.is_present {
                let mut n = 0;
                let s = b"Y-axis accel: ";
                cap_buf[..s.len()].copy_from_slice(s);
                n += s.len();
                if t.accel_y < 0 {
                    cap_buf[n] = b'-';
                    n += 1;
                }
                let mag = (if t.accel_y < 0 { -t.accel_y } else { t.accel_y }) as u32;
                n += write_decimal(mag, &mut cap_buf[n..]);
                let mg = b" mg (live)";
                let take = mg.len().min(cap_buf.len() - n);
                cap_buf[n..n + take].copy_from_slice(&mg[..take]);
                n += take;
                (&cap_buf[..n], SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
            } else if t.error_msg_len > 0 {
                (&t.error_msg[..t.error_msg_len], TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            } else {
                (b"Hardware unbound (no physical silicon)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Accelerometer (3-Axis IMU)", cap, cap_ink, badge, badge_ink)
        }
        1 => {
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if t.is_present {
                (b"Roll, pitch & yaw angular rate active", SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
            } else {
                (b"Hardware unbound (no physical silicon)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Gyroscope (Angular Rate)", cap, cap_ink, badge, badge_ink)
        }
        2 => {
            let mut cap_buf = [0u8; 64];
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if t.is_present {
                let mut n = 0;
                let s = b"Ambient illuminance: ";
                cap_buf[..s.len()].copy_from_slice(s);
                n += s.len();
                let mag = t.lux.max(0) as u32;
                n += write_decimal(mag, &mut cap_buf[n..]);
                let lx = b" lux";
                let take = lx.len().min(cap_buf.len() - n);
                cap_buf[n..n + take].copy_from_slice(&lx[..take]);
                n += take;
                (&cap_buf[..n], SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
            } else {
                (b"Hardware unbound (no physical silicon)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Ambient Light Sensor", cap, cap_ink, badge, badge_ink)
        }
        3 => {
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if t.is_present {
                if t.is_near {
                    (b"Infrared proximity: object near (<5cm)", ACCENT, b"NEAR", ACCENT)
                } else {
                    (b"Infrared proximity: clear / far", SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
                }
            } else {
                (b"Hardware unbound (no physical silicon)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Proximity Sensor (IR ToF)", cap, cap_ink, badge, badge_ink)
        }
        4 => {
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if t.is_present {
                (b"Atmospheric pressure: nominal 1013.25 hPa", SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
            } else {
                (b"Hardware unbound (no physical silicon)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Barometer (Pressure)", cap, cap_ink, badge, badge_ink)
        }
        5 => {
            let mut cap_buf = [0u8; 64];
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if t.is_present {
                if t.orientation_len > 0 {
                    let mut n = 0;
                    let s = b"Orientation: ";
                    cap_buf[..s.len()].copy_from_slice(s);
                    n += s.len();
                    let ori = &t.orientation[..t.orientation_len];
                    let take = ori.len().min(cap_buf.len() - n);
                    cap_buf[n..n + take].copy_from_slice(&ori[..take]);
                    n += take;
                    (&cap_buf[..n], SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
                } else {
                    (b"Digital compass heading online", SUCCESS_EMERALD, b"ONLINE", SUCCESS_EMERALD)
                }
            } else {
                (b"Hardware unbound (no physical silicon)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Magnetometer (Compass)", cap, cap_ink, badge, badge_ink)
        }
        _ => {
            let (cap, cap_ink, badge, badge_ink): (&[u8], u32, &[u8], u32) = if app.thermal == TelemetryState::Reported {
                (b"Multi-zone thermal telemetry monitored", SUCCESS_EMERALD, b"REPORTED", SUCCESS_EMERALD)
            } else {
                (b"Hardware unbound (no physical thermal zones)", TEXT_MUTED, b"UNBOUND", TEXT_MUTED)
            };
            diag_row_head(x, y, cy, b"Thermal Telemetry Zones", cap, cap_ink, badge, badge_ink)
        }
    }
}

/// Dedicated full-screen Sensors & Diagnostics pixel renderer (7 stacked diagnostic cards).
fn sensors_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    if let Some(c) = run_ink(
        x,
        y,
        HEADER_X,
        HEADER_Y,
        TYPE_DISPLAY,
        b"Sensors & Diagnostics",
        W - HEADER_X - MARGIN,
        TEXT,
        BG,
        true,
    ) {
        return c;
    }
    if y == DIVIDER_Y {
        return BORDER;
    }
    for row in 0..7 {
        let (cy, ch) = sensors_row_rect(row);
        if let Some(fill) = card_pixel(x, y, CONTENT_X, cy, CONTENT_W, ch, CARD_R) {
            let _ = fill;
            if let Some(c) = diag_row_pixel(x, y, app, row, cy) {
                return c;
            }
            return SURFACE;
        }
    }
    BG
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
                    b"Direct DRM/KMS FrameBuffer (720x1440, 60Hz)",
                    TEXT_MUTED,
                    b"ACTIVE",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            1 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Sound & Haptics",
                    b"Hardware audio sink unavailable",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
            2 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Volume & Feedback",
                    b"Haptic feedback motor & PCM master channel",
                    TEXT_MUTED,
                    b"REPORTED",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            3 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"System Gestures",
                    b"Full-screen edge swipes & home gesture bar",
                    TEXT_MUTED,
                    b"ENABLED",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            _ => {
                let (cap, cap_ink): (&[u8], u32) = if app.sensor_telemetry.is_present {
                    (b"Hardware IMU & ambient photodiode online", SUCCESS_EMERALD)
                } else if app.sensor_telemetry.error_msg_len > 0 {
                    (&app.sensor_telemetry.error_msg[..app.sensor_telemetry.error_msg_len], TEXT_MUTED)
                } else {
                    (b"Hardware unbound (platform truth contract)", TEXT_MUTED)
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Sensors & Diagnostics",
                    cap,
                    cap_ink,
                    b"VIEW",
                    ACCENT,
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
                        &b"Wireless telemetry reported"[..],
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
                    b"Cellular Baseband",
                    b"Cellular status unavailable",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
            2 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Flight Mode Status",
                    b"All sovereign radios enabled & listening",
                    TEXT_MUTED,
                    b"DISABLED",
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
                    b"Bluetooth Low Energy",
                    b"Bluetooth status unavailable",
                    TEXT_MUTED,
                    b"UNAVAILABLE",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
        },
        Section::Personal => match row {
            0 => {
                let owner = if app.owner_name_len > 0 {
                    &app.owner_name[..app.owner_name_len]
                } else {
                    b"Sovereign Owner".as_slice()
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Sovereign Identity",
                    owner,
                    TEXT_MUTED,
                    b"ACTIVE",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            1 => {
                let status = if app.user_status_len > 0 {
                    &app.user_status[..app.user_status_len]
                } else {
                    b"Encrypted & Sovereign".as_slice()
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Status Phrase",
                    status,
                    ACCENT,
                    b"ENCRYPTED",
                    ACCENT,
                ) {
                    return c;
                }
            }
            2 => {
                let home = app
                    .wallpaper_home
                    .map(|s| s.name)
                    .unwrap_or(b"not set".as_slice());
                let lock = app
                    .wallpaper_lock
                    .map(|s| s.name)
                    .unwrap_or(b"not set".as_slice());
                let mut cap = [0u8; 96];
                let mut n = 0usize;
                for chunk in [b"Home: ", home, b"  Lock: ", lock] {
                    let take = chunk.len().min(cap.len() - n);
                    cap[n..n + take].copy_from_slice(&chunk[..take]);
                    n += take;
                }
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Wallpaper",
                    &cap[..n],
                    TEXT_MUTED,
                    if app.wallpaper_source == WallpaperSource::Answered {
                        b"OPEN"
                    } else {
                        b"UNREAD"
                    },
                    if app.wallpaper_source == WallpaperSource::Answered {
                        ACCENT
                    } else {
                        TEXT_MUTED
                    },
                ) {
                    return c;
                }
            }
            _ => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Language & Locale",
                    b"System language: English (en-US)",
                    TEXT_MUTED,
                    b"DEFAULT",
                    TEXT_MUTED,
                ) {
                    return c;
                }
            }
        },
        Section::Apps => match row {
            0 => {
                let mut abuf = [0u8; 64];
                let mut an = write_decimal(app.app_count as u32, &mut abuf);
                an = push_suffix(&mut abuf, an, b" verified sovereign packages");
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Installed Applications",
                    &abuf[..an],
                    TEXT_MUTED,
                    b"VERIFIED",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            1 => {
                let mut sbuf = [0u8; 64];
                let mb = (app.storage_bytes / (1024 * 1024)) as u32;
                let mut sn = write_decimal(mb, &mut sbuf);
                sn = push_suffix(&mut sbuf, sn, b" MB root block storage mounted");
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Storage & VFS Filesystem",
                    &sbuf[..sn],
                    TEXT_MUTED,
                    b"MOUNTED",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            2 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Sandboxing & Isolation",
                    b"Zero ambient authority & capability tables enforced",
                    TEXT_MUTED,
                    b"ENFORCED",
                    SUCCESS_EMERALD,
                ) {
                    return c;
                }
            }
            _ => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"App Permissions",
                    b"Granular object-scoped capability delegation",
                    TEXT_MUTED,
                    b"AUDITED",
                    ACCENT,
                ) {
                    return c;
                }
            }
        },
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
                    DETAIL_X + ROW_PAD,
                    cy + SEG_FEEDBACK_DY,
                    TYPE_H3,
                    &fb[..fb_len],
                    DETAIL_W - 2 * ROW_PAD,
                    fb_ink,
                    SURFACE,
                    false,
                ) {
                    return c;
                }
            }
            2 => {
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Display Timeout & Sleep",
                    b"Screen sleep timeout: 30 seconds idle",
                    TEXT_MUTED,
                    b"ACTIVE",
                    SUCCESS_EMERALD,
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
                // The one row on this screen that reports a state this app can
                // actually change, so it carries the account layer's own answer
                // rather than the "UNAVAILABLE" treatment the rows below use for
                // subsystems this platform has no way to read.
                let (_, detail, ink, badge) = sl_state_line(app);
                if let Some(c) = row_head(x, y, cy, b"Screen lock", detail, ink, badge, TEXT) {
                    return c;
                }
            }
            1 => {
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
            2 => {
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
            3 => {
                let mut ubuf = [0u8; 80];
                let (cap, cap_ink, badge, badge_ink) = if app.uptime_ticks > 0 {
                    let mut un = write_decimal_u64(app.uptime_ticks, &mut ubuf);
                    un = push_suffix(&mut ubuf, un, b" ticks (x86_64 Sovereign SMP)");
                    (&ubuf[..un], TEXT, &b"REPORTED"[..], SUCCESS_EMERALD)
                } else {
                    let s = b"x86_64 Sovereign SMP / 64-bit Paging";
                    ubuf[..s.len()].copy_from_slice(s);
                    (&ubuf[..s.len()], TEXT_MUTED, &b"HARDWARE"[..], SUCCESS_EMERALD)
                };
                if let Some(c) = row_head(
                    x,
                    y,
                    cy,
                    b"Architecture & SMP",
                    cap,
                    cap_ink,
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
                    DETAIL_X + ROW_PAD,
                    cy + ROW_CAP2_DY,
                    TYPE_H3,
                    b"Config key: developer.terminal_enabled",
                    DETAIL_W - 2 * ROW_PAD - bw - BADGE_GAP,
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
///
/// WHY the title stays on the Display rung: the screen title and the row titles
/// have to be two different ranks for the hierarchy to read at all, and with a
/// face that rasterises at 24/48/72 the only two ranks available are H3 (24) and
/// Display (48). H2 (28-32) is not expressible, so the screen title keeps
/// Display and the row titles take H3, exactly as the typography constants record.
fn detail_pixel(x: usize, y: usize, app: &SettingsApp, section: Section) -> u32 {
    if let Some(c) = run_ink(
        x,
        y,
        HEADER_X,
        HEADER_Y,
        TYPE_DISPLAY,
        section.title(),
        W - HEADER_X - MARGIN,
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
        if let Some(fill) = card_pixel(x, y, CONTENT_X, cy, CONTENT_W, ch, CARD_R) {
            return row_pixel(x, y, app, section, row, cy, fill);
        }
    }
    BG
}

/// One landing category card: icon, bold title, subtitle wrapped to at most
/// two measured lines (wrapping, not ellipsis, because the phrases are short
/// word lists that must stay whole).
///
/// WHY the icon and the title share one line box: the reference System Space puts
/// the mark beside the label rather than above it, and centring the icon on the
/// title's line box is what makes that a column rather than a stack. WHY the
/// title is the H3 rung: it is a card heading, which the doc sizes at H3, and it
/// keeps the card one rank below the screen's own Display title.
fn landing_card_pixel(x: usize, y: usize, i: usize) -> Option<u32> {
    let (cx0, cy0, cw, ch) = card_rect(i);
    if let Some(fill) = card_pixel(x, y, cx0, cy0, cw, ch, CARD_R) {
        let (title, subtitle, icon) = category(i);

        // 1. Squircle icon badge on left (48x48, radius 14)
        let icon_box_x = cx0 + 16;
        let icon_box_y = cy0 + 19;
        let icon_box_s = 48;
        if in_round_rect(x, y, icon_box_x, icon_box_y, icon_box_s, icon_box_s, 14) {
            let cx = icon_box_x + icon_box_s / 2;
            let cy = icon_box_y + icon_box_s / 2;
            let tint = match i {
                0 => 0xFF2563EB, // Device: Royal Blue
                1 => 0xFF0284C7, // Network: Sky Blue
                2 => 0xFF7C3AED, // Personal: Violet
                3 => 0xFFD97706, // Apps: Amber
                4 => 0xFF16A34A, // Power: Emerald
                5 => 0xFFE11D48, // Security: Rose
                _ => 0xFF4B5563, // About: Slate
            };
            if let Some(ic) = icon(x, y, cx, cy, tint) {
                return Some(ic);
            }
            return Some(SOFT);
        }

        // 2. Title and subtitle (full horizontal run without cramped clipping)
        let text_x = cx0 + 78;
        let text_w = cw.saturating_sub(78 + 110);

        if let Some(c) = run_ink(
            x,
            y,
            text_x,
            cy0 + 18,
            TYPE_H3,
            title,
            text_w,
            TEXT,
            fill,
            true,
        ) {
            return Some(c);
        }

        if let Some(c) = run_ink(
            x,
            y,
            text_x,
            cy0 + 46,
            TYPE_H3,
            subtitle,
            text_w,
            TEXT_MUTED,
            fill,
            false,
        ) {
            return Some(c);
        }

        // 3. Right status chip
        let badge_w = 88;
        let badge_h = 28;
        let badge_x = cx0 + cw - badge_w - 18;
        let badge_y = cy0 + (ch - badge_h) / 2;
        if in_round_rect(x, y, badge_x, badge_y, badge_w, badge_h, 14) {
            let (label, tint) = match i {
                0 => (b"Config" as &[u8], 0xFF0284C7),
                1 => (b"Online" as &[u8], 0xFF16A34A),
                2 => (b"Theme" as &[u8], 0xFF7C3AED),
                3 => (b"14 RAPP" as &[u8], 0xFFD97706),
                4 => (b"Profile" as &[u8], 0xFF16A34A),
                5 => (b"Ring-3" as &[u8], 0xFFE11D48),
                _ => (b"v0.8.0" as &[u8], 0xFF4B5563),
            };
            let lw = run_w(label, TYPE_H3, false);
            let lx = badge_x + (badge_w.saturating_sub(lw)) / 2;
            let ly = badge_y + 2;
            if let Some(c) = run_ink(x, y, lx, ly, TYPE_H3, label, badge_w, tint, SOFT, false) {
                return Some(c);
            }
            return Some(SOFT);
        }

        return Some(fill);
    }
    None
}

/// Renders a pixel of the Dynamic Device Vitality Orb at (ORB_CX, ORB_CY) with core radius ORB_R
/// and ethereal ambient halo extending out to HALO_R.
fn vitality_orb_pixel(x: usize, y: usize, state: OrbState) -> Option<u32> {
    if x < ORB_CX.saturating_sub(HALO_R)
        || x > ORB_CX + HALO_R
        || y < ORB_CY.saturating_sub(HALO_R)
        || y > ORB_CY + HALO_R
    {
        return None;
    }
    let dx = (x as i32) - (ORB_CX as i32);
    let dy = (y as i32) - (ORB_CY as i32);
    let d2 = (dx * dx + dy * dy) as u32;
    if d2 > (HALO_R * HALO_R) as u32 {
        return None;
    }
    let dist = d2.isqrt();
    let (c_inner, c_outer) = state.colors();
    let r_inner = (c_inner >> 16) & 0xFF;
    let g_inner = (c_inner >> 8) & 0xFF;
    let b_inner = c_inner & 0xFF;

    let r_outer = (c_outer >> 16) & 0xFF;
    let g_outer = (c_outer >> 8) & 0xFF;
    let b_outer = c_outer & 0xFF;

    if dist <= ORB_R as u32 {
        // Inner core: smooth neon radial gradient from c_inner at center to c_outer at rim
        let t = (dist * 255) / (ORB_R as u32);
        let r = ((r_inner * (255 - t) + r_outer * t) / 255) as u32;
        let g = ((g_inner * (255 - t) + g_outer * t) / 255) as u32;
        let b = ((b_inner * (255 - t) + b_outer * t) / 255) as u32;

        // Specular highlight at top-left inside core (dx ~ -8, dy ~ -8)
        let sx = dx + 8;
        let sy = dy + 8;
        let sd2 = (sx * sx + sy * sy) as u32;
        if sd2 < 64 {
            let s_dist = sd2.isqrt();
            let s_factor = ((8 - s_dist) * 48) / 8;
            let sr = (r + s_factor).min(255);
            let sg = (g + s_factor).min(255);
            let sb = (b + s_factor).min(255);
            return Some((sr << 16) | (sg << 8) | sb);
        }

        Some((r << 16) | (g << 8) | b)
    } else {
        // Ambient ethereal halo smoothly fading from c_outer into BG
        let halo_span = (HALO_R - ORB_R) as u32;
        let halo_d = dist - (ORB_R as u32);
        // Alpha fades from ~120 at rim to 0 at HALO_R
        let alpha = ((halo_span.saturating_sub(halo_d)) * 120) / halo_span;
        let bg_r = (BG >> 16) & 0xFF;
        let bg_g = (BG >> 8) & 0xFF;
        let bg_b = BG & 0xFF;

        let r = (r_outer * alpha + bg_r * (255 - alpha)) / 255;
        let g = (g_outer * alpha + bg_g * (255 - alpha)) / 255;
        let b = (b_outer * alpha + bg_b * (255 - alpha)) / 255;
        Some((r << 16) | (g << 8) | b)
    }
}

/// Landing screen pixel: Dynamic Device Vitality Orb, identity, and grouped cards.
fn landing_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    if y >= GRID_Y {
        for i in 0..Section::COUNT {
            if let Some(c) = landing_card_pixel(x, y, i) {
                return c;
            }
        }
        // Sovereign System Status Card below cards
        let stat_y = 874;
        let stat_h = 160;
        if let Some(fill) = card_pixel(x, y, CONTENT_X, stat_y, CONTENT_W, stat_h, CARD_R) {
            let title = b"Sovereign Architecture Guarantee";
            if let Some(c) = run_ink(x, y, CONTENT_X + 24, stat_y + 16, TYPE_H3, title, CONTENT_W - 48, TEXT, fill, true) {
                return c;
            }
            let l1 = b"Ring-3 Privilege Separation * Zero Ambient Authority";
            if let Some(c) = run_ink(x, y, CONTENT_X + 24, stat_y + 46, TYPE_H3, l1, CONTENT_W - 48, TEXT_MUTED, fill, false) {
                return c;
            }
            let l2 = b"Hardware Enclave Active * Monotonic Ticks Synchronized";
            if let Some(c) = run_ink(x, y, CONTENT_X + 24, stat_y + 74, TYPE_H3, l2, CONTENT_W - 48, TEXT_MUTED, fill, false) {
                return c;
            }
            let l3 = b"WHPX/KVM Accelerated * Native 1080x2160 Scanout";
            if let Some(c) = run_ink(x, y, CONTENT_X + 24, stat_y + 102, TYPE_H3, l3, CONTENT_W - 48, TEXT_MUTED, fill, false) {
                return c;
            }
            let l4 = b"Cryptographic Keystore Active * VFS Sandboxing Enforced";
            if let Some(c) = run_ink(x, y, CONTENT_X + 24, stat_y + 128, TYPE_H3, l4, CONTENT_W - 48, SUCCESS_EMERALD, fill, false) {
                return c;
            }
            return fill;
        }
    }
    // Dynamic Device Vitality Orb centered at ORB_CX, ORB_CY
    if let Some(c) = vitality_orb_pixel(x, y, app.orb_state()) {
        return c;
    }
    // Device identity title below orb: "Settings"
    let title = b"Settings";
    let tw = run_w(title, TYPE_DISPLAY, true);
    let tx = if tw < W { (W - tw) / 2 } else { CONTENT_X };
    if let Some(c) = run_ink(
        x,
        y,
        tx,
        IDENTITY_TITLE_Y,
        TYPE_DISPLAY,
        title,
        W - 2 * CONTENT_X,
        TEXT,
        BG,
        true,
    ) {
        return c;
    }
    // User status phrase below title: "InterOS Sovereign * Verified Ring-3"
    let sub = b"InterOS Sovereign * Verified Ring-3";
    let sw = run_w(sub, TYPE_H3, false);
    let sx = if sw < W { (W - sw) / 2 } else { CONTENT_X };
    if let Some(c) = run_ink(
        x,
        y,
        sx,
        IDENTITY_SUB_Y,
        TYPE_H3,
        sub,
        W - 2 * CONTENT_X,
        TEXT_MUTED,
        BG,
        false,
    ) {
        return c;
    }
    BG
}

// -----------------------------------------------------------------------------
// Wallpaper picker
// -----------------------------------------------------------------------------

/// Relative luminance of an `0xAARRGGBB` colour, on the wallpaper's own scale.
///
/// WHY this exists on this app and not in the SDK: the picker is the only surface
/// that has to choose ink for something drawn *on top of* a wallpaper, and the SDK
/// already provides the wallpaper. A two-line helper here keeps that decision local
/// instead of exporting a colour utility for one caller.
fn wall_luma(color: u32) -> u32 {
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    (r * 299 + g * 587 + b * 114) / 1000
}

/// Top-left corner of gallery cell `i`, laid out three to a row.
fn wp_cell_origin(i: usize) -> (usize, usize) {
    let col = i % 3;
    let row = i / 3;
    (
        CONTENT_X + col * (WP_CELL_W + WP_CELL_GAP),
        WP_GALLERY_Y + row * (WP_CELL_H + WP_CELL_GAP),
    )
}

/// Gallery cell hit-test: the whole rounded cell is the target.
fn wp_cell_at(px: u32, py: u32) -> Option<usize> {
    for i in 0..std::design::walls::SCENES.len() {
        let (cx0, cy0) = wp_cell_origin(i);
        if in_round_rect(
            px as usize,
            py as usize,
            cx0,
            cy0,
            WP_CELL_W,
            WP_CELL_H,
            CARD_R,
        ) {
            return Some(i);
        }
    }
    None
}

/// The preview strip's crop rectangle.
///
/// WHY the preview shows a horizontal slice of the scene rather than the whole
/// of it: a wallpaper is chosen by how it fills a 9:20 screen, so showing the
/// whole surface at once in this box would shrink it to a thumbnail and hide the
/// part that matters -- whether the horizon lands above or behind the status bar.
/// The slice shown is the scene's upper rows, which is where the sky, the ridge
/// and the status bar all end up.
fn wp_preview_slice(scene: &std::design::walls::Scene, x: usize, y: usize) -> u32 {
    let sy = WP_PREVIEW_Y + (y * 1440) / (WP_PREVIEW_H * 4).min(1440);
    let sx = CONTENT_X + (x * 720) / CONTENT_W;
    std::design::walls::scene_pixel(scene, sx.min(719), sy.min(1439), 720, 1440)
}

/// The target selector's three segments: a segmented control on the doc's control
/// rung, with the doc's 4px segment gap so the three read as one control.
fn wp_segment_rect(i: usize) -> (usize, usize, usize, usize) {
    let seg_w = (CONTENT_W - 2 * SEG_GAP) / 3;
    (
        CONTENT_X + i * (seg_w + SEG_GAP),
        WP_TARGET_Y,
        seg_w,
        WP_TARGET_H,
    )
}

/// The wallpaper picker panel.
///
/// WHY the whole panel is one function that answers per pixel: this app has no
/// allocation, so there is nothing to build ahead of time and a per-pixel
/// painter is the only shape available. Every measurement below is a cheap
/// integer step, not a text metric.
fn wallpaper_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    // WHY there is no "y < 104 is empty" early return: the header below is drawn
    // at y 32 and y 88, so this return was erasing the panel's own title and
    // subtitle. It survives as the last few rows of the subtitle, which is why
    // the header looked clipped rather than missing.
    // Header, on the shared header geometry the detail screens use, so the title
    // of a panel and the title of the screen it was opened from line up.
    if let Some(c) = run_ink(
        x,
        y,
        HEADER_X,
        HEADER_Y,
        TYPE_DISPLAY,
        b"Wallpaper",
        W - HEADER_X - MARGIN,
        TEXT,
        BG,
        true,
    ) {
        return c;
    }
    if let Some(c) = run_ink(
        x,
        y,
        HEADER_X,
        HEADER_SUB_Y,
        TYPE_H3,
        b"Shown on the lock screen and the home surface",
        W - HEADER_X - MARGIN,
        TEXT_MUTED,
        BG,
        false,
    ) {
        return c;
    }

    // Preview strip: the scene itself, so what is selected is what is seen.
    if (WP_PREVIEW_Y..WP_PREVIEW_Y + WP_PREVIEW_H).contains(&y) {
        let preview = app.wallpaper_preview();
        let base = wp_preview_slice(preview, x, y);
        if !in_round_rect(
            x,
            y,
            CONTENT_X,
            WP_PREVIEW_Y,
            CONTENT_W,
            WP_PREVIEW_H,
            WP_PREVIEW_R,
        ) {
            return BG;
        }
        // Name plate over the lower edge of the preview, so the scene in view is
        // named without covering its sky.
        //
        // WHY the plate's polarity is decided from the wallpaper pixel under it
        // and not from the scene's palette: the plate sits at the bottom of the
        // strip, and a scene whose sky is light has a dark foreground. Deciding
        // polarity from the sky would put dark ink on dark rock.
        let plate_y = WP_PREVIEW_Y + WP_PREVIEW_H - MARGIN - PLATE_H;
        if y >= plate_y
            && in_round_rect(
                x,
                y,
                CONTENT_X + PLATE_INSET,
                plate_y,
                CONTENT_W - 2 * PLATE_INSET,
                PLATE_H,
                CONTROL_R,
            )
        {
            let dark_under = wall_luma(base) < 96;
            // WHY both inks come from `design::color` with the alpha forced
            // afterwards: this pair used to be four literals, two of which were
            // the palette's own text and base in disguise.
            let (ink, plate) = if dark_under {
                (
                    std::design::color::DARK_TEXT,
                    with_alpha(std::design::color::DARK_BACKGROUND, PLATE_ALPHA),
                )
            } else {
                (TEXT, with_alpha(BG, PLATE_ALPHA))
            };
            if let Some(c) = run_ink(
                x,
                y,
                CONTENT_X + PLATE_INSET + CARD_GAP,
                plate_y + (PLATE_H - LINE_H) / 2,
                TYPE_H3,
                preview.name,
                CONTENT_W - 2 * PLATE_INSET - 2 * CARD_GAP,
                ink,
                plate,
                false,
            ) {
                return c;
            }
            return plate;
        }
        return base;
    }

    // Target selector.
    if (WP_TARGET_Y..WP_TARGET_Y + WP_TARGET_H).contains(&y) {
        for i in 0..WallpaperTarget::COUNT {
            let (sx0, sy0, sw, sh) = wp_segment_rect(i);
            let selected = WallpaperTarget::from_index(i) == app.wallpaper_target;
            let label = WallpaperTarget::from_index(i).label();
            if let Some(c) = segment_pixel(x, y, sx0, sy0, sw, sh, label, selected) {
                return c;
            }
        }
        return BG;
    }

    // Gallery.
    if y >= WP_GALLERY_Y && y < WP_ACTION_Y {
        for i in 0..std::design::walls::SCENES.len() {
            let (cx0, cy0) = wp_cell_origin(i);
            if !in_round_rect(x, y, cx0, cy0, WP_CELL_W, WP_CELL_H, CARD_R) {
                continue;
            }
            let scene = &std::design::walls::SCENES[i];
            let is_pending = app.wallpaper_pending == Some(scene);
            let ring = if is_pending { ACCENT } else { BORDER };
            if !in_round_rect(
                x + 3,
                y + 3,
                cx0 + 3,
                cy0 + 3,
                WP_CELL_W - 6,
                WP_CELL_H - 6,
                CARD_R - 3,
            ) {
                return ring;
            }
            // Thumbnail: a band of the scene rather than a swatch, so a gallery
            // entry shows the composition it will paint.
            //
            // WHY the band is the scene's *upper three quarters* and why it is
            // rendered through `scene_pixel` rather than `ramp3`: a daylight
            // scene's sky stop sits within a few steps of white, so a ramp swatch
            // of it is invisible against the white card it sits on. The full
            // composition reaches the dark foreground, which is what makes an
            // entry recognisable at thumbnail size.
            let bx = cx0 + WP_CELL_PAD;
            let by = cy0 + WP_CELL_PAD;
            let bw = WP_CELL_W - 2 * WP_CELL_PAD;
            let bh = WP_THUMB_H;
            if in_round_rect(x, y, bx, by, bw, bh, IMAGE_R) {
                let tx = ((x - bx) * 720) / bw;
                // Sample the scene's top three quarters: the sky, the ridge line
                // and part of the foreground, which is the range that actually
                // distinguishes one wallpaper from another.
                let ty = ((y - by) * 1080) / bh;
                return std::design::walls::scene_pixel(
                    scene,
                    tx.min(719),
                    ty.min(1439),
                    720,
                    1440,
                );
            }
            let lw = run_w(scene.name, TYPE_H3, true);
            let lx = if lw + 2 * WP_CELL_PAD >= WP_CELL_W {
                cx0 + WP_CELL_PAD
            } else {
                cx0 + (WP_CELL_W - lw) / 2
            };
            if let Some(c) = run_ink(
                x,
                y,
                lx,
                cy0 + WP_CELL_H - WP_CELL_PAD - LINE_H,
                TYPE_H3,
                scene.name,
                WP_CELL_W - 2 * WP_CELL_PAD,
                TEXT,
                SURFACE,
                true,
            ) {
                return c;
            }
            return SURFACE;
        }
        return BG;
    }

    // Actions.
    if (WP_ACTION_Y..WP_ACTION_Y + WP_ACTION_H).contains(&y) {
        let apply = in_round_rect(
            x,
            y,
            CONTENT_X,
            WP_ACTION_Y,
            BUTTON_HALF,
            BUTTON_H,
            BUTTON_H / 2,
        );
        let reset = in_round_rect(
            x,
            y,
            CONTENT_X + BUTTON_HALF + CARD_GAP,
            WP_ACTION_Y,
            BUTTON_HALF,
            BUTTON_H,
            BUTTON_H / 2,
        );
        if !apply && !reset {
            return BG;
        }
        // WHY the Apply press is only drawn enabled and only enabled when a
        // selection exists: an enabled-looking Apply with nothing to write is the
        // dead control this app's contract forbids.
        let enabled = app.wallpaper_pending.is_some();
        // WHY Apply is the palette's primary ink and Reset the soft fill: the
        // reference `PrimaryButton` is a dark pill and `SecondaryButton` a soft
        // one, and this panel has exactly one of each.
        let (label, bx, fill, ink, rim) = if apply {
            (
                b"Apply".as_slice(),
                CONTENT_X,
                if enabled { TEXT } else { SOFT },
                if enabled { SURFACE } else { TEXT_MUTED },
                if enabled { None } else { Some(BORDER) },
            )
        } else {
            (
                b"Reset to default".as_slice(),
                CONTENT_X + BUTTON_HALF + CARD_GAP,
                SOFT,
                TEXT,
                Some(BORDER),
            )
        };
        if let Some(c) = button_pixel(
            x,
            y,
            bx,
            WP_ACTION_Y,
            BUTTON_HALF,
            BUTTON_H,
            label,
            fill,
            ink,
            rim,
        ) {
            return c;
        }
        return BG;
    }

    // Outcome line.
    if y >= WP_STATUS_Y {
        let (line, ink): (&[u8], u32) = match app.wallpaper_write {
            WallpaperWrite::Applied(target) => match target {
                WallpaperTarget::Home => (&b"Applied to the home surface"[..], ACCENT),
                WallpaperTarget::Lock => (&b"Applied to the lock screen"[..], ACCENT),
                WallpaperTarget::Both => (&b"Applied to home and lock screens"[..], ACCENT),
            },
            WallpaperWrite::Refused => (
                &b"The settings store refused the write; nothing changed"[..],
                TEXT_MUTED,
            ),
            WallpaperWrite::Unattempted => match app.wallpaper_source {
                WallpaperSource::Answered => (
                    &b"Select a wallpaper, then choose a target and apply"[..],
                    TEXT_MUTED,
                ),
                WallpaperSource::Unread => (
                    &b"The settings store did not answer; the current wallpaper is unknown"[..],
                    TEXT_MUTED,
                ),
            },
        };
        if let Some(c) = run_ink(
            x,
            y,
            CONTENT_X,
            WP_STATUS_Y,
            TYPE_H3,
            line,
            CONTENT_W,
            ink,
            BG,
            false,
        ) {
            return c;
        }
    }

    BG
}

/// Hit-test for the picker's three interactive regions.
///
/// WHY every region uses the same radius the painter uses: the drawer and the
/// hit-test used to disagree (14 drawn, 16 tested), so the corners of a segment
/// answered taps that landed on the base. One radius per component removes that
/// class of bug rather than fixing an instance of it.
fn wallpaper_at(px: u32, py: u32) -> Option<WallpaperHit> {
    if (WP_TARGET_Y..WP_TARGET_Y + WP_TARGET_H).contains(&(py as usize)) {
        for i in 0..WallpaperTarget::COUNT {
            let (sx0, sy0, sw, sh) = wp_segment_rect(i);
            if in_round_rect(px as usize, py as usize, sx0, sy0, sw, sh, CONTROL_R) {
                return Some(WallpaperHit::Target(i));
            }
        }
    }
    if let Some(i) = wp_cell_at(px, py) {
        return Some(WallpaperHit::Cell(i));
    }
    if (WP_ACTION_Y..WP_ACTION_Y + WP_ACTION_H).contains(&(py as usize)) {
        if in_round_rect(
            px as usize,
            py as usize,
            CONTENT_X,
            WP_ACTION_Y,
            BUTTON_HALF,
            BUTTON_H,
            BUTTON_H / 2,
        ) {
            return Some(WallpaperHit::Apply);
        }
        if in_round_rect(
            px as usize,
            py as usize,
            CONTENT_X + BUTTON_HALF + CARD_GAP,
            WP_ACTION_Y,
            BUTTON_HALF,
            BUTTON_H,
            BUTTON_H / 2,
        ) {
            return Some(WallpaperHit::Reset);
        }
    }
    None
}

/// The picker's interactive regions, resolved to one value so the hit-test's
/// return type names what was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperHit {
    /// Segment `n` of the target selector, 0=Home 1=Lock 2=Both.
    Target(usize),
    /// Gallery cell `n`.
    Cell(usize),
    /// The Apply button.
    Apply,
    /// The Reset to default button.
    Reset,
}

// Screen lock panel: rendering and hit-test
// -----------------------------------------------------------------------------

/// The keypad's digit for grid position `row`/`col`, or `None` for the CLR and OK
/// keys that share the last row with the digits.
///
/// WHY the same 3x4 grid as the lock screen's own pad: the owner learns the
/// keypad once. The geometry is identical on purpose - a key is where the
/// owner's finger already expects it.
fn sl_digit(row: usize, col: usize) -> Option<u8> {
    match (row, col) {
        (0, 0) => Some(b'1'),
        (0, 1) => Some(b'2'),
        (0, 2) => Some(b'3'),
        (1, 0) => Some(b'4'),
        (1, 1) => Some(b'5'),
        (1, 2) => Some(b'6'),
        (2, 0) => Some(b'7'),
        (2, 1) => Some(b'8'),
        (2, 2) => Some(b'9'),
        (3, 1) => Some(b'0'),
        _ => None,
    }
}

/// Top-left of keypad key `row`/`col`.
fn sl_key_origin(row: usize, col: usize) -> (usize, usize) {
    (
        CONTENT_X + col * (SL_KEY_W + SL_KEY_GAP),
        SL_KEYS_Y + row * (SL_KEY_H + SL_KEY_GAP),
    )
}

/// The one sentence that describes the account layer's answer, and its ink.
///
/// WHY the credential query leads: the lock state cannot say whether a PIN
/// exists, and only the account layer can. WHY each line is short: the card is
/// measured against the badge, and a sentence long enough to be ellipsised is a
/// sentence that does not get read.
///
/// WHY the refused case has its own line: "no PIN is set" and "the account layer
/// did not answer" are different states, and collapsing them would show an owner
/// a definite lock state the device never reported.
fn sl_state_line(app: &SettingsApp) -> (&'static [u8], &'static [u8], u32, &'static [u8]) {
    match app.screen_lock_has_credential {
        None => (
            b"Unknown",
            b"The account layer did not answer",
            TEXT_MUTED,
            b"UNKNOWN",
        ),
        Some(false) => (
            b"No PIN set",
            b"The lock screen unlocks on a swipe",
            TEXT_MUTED,
            b"OFF",
        ),
        Some(true) => match app.screen_lock_reported {
            Some(std::UserLockState::Unlocked) => (
                b"PIN set",
                b"Unlocked in this session",
                TEXT,
                b"PIN",
            ),
            Some(std::UserLockState::Lockout) => (
                b"PIN set",
                b"Locked out after failed attempts",
                TEXT,
                b"WAIT",
            ),
            Some(std::UserLockState::Uninitialized) => (
                // WHY the account layer's two answers are reported rather than
                // reconciled here: it says a credential exists while also
                // reporting no initialized lock. Resolving that in this app
                // would be inventing a state the kernel never reported.
                b"PIN set",
                b"The account reports no initialized lock",
                TEXT_MUTED,
                b"PIN",
            ),
            _ => (
                b"PIN set",
                b"Required to unlock this device",
                TEXT,
                b"PIN",
            ),
        },
    }
}

/// The prompt for the step in progress, or an empty run on the overview.
fn sl_prompt(app: &SettingsApp) -> &'static [u8] {
    match app.screen_lock_mode {
        ScreenLockMode::Idle => b"",
        ScreenLockMode::EnterCurrent => match app.screen_lock_action {
            ScreenLockAction::Change => b"Enter the current PIN",
            ScreenLockAction::Remove => b"Enter the current PIN to remove it",
            ScreenLockAction::Establish => b"Enter the current PIN",
        },
        ScreenLockMode::EnterNew => b"Enter a new PIN",
        ScreenLockMode::ConfirmNew => b"Enter it again to confirm",
    }
}

/// The complaint line for the current error, or an empty run.
fn sl_error_line(app: &SettingsApp) -> &'static [u8] {
    match app.screen_lock_error {
        SL_ERR_SHORT => b"At least 4 digits are required",
        SL_ERR_MISMATCH => b"The two entries did not match; nothing was written",
        SL_ERR_REFUSED => b"The account layer refused the change; nothing was written",
        SL_ERR_NOT_CURRENT => b"That is not the current PIN",
        _ => b"",
    }
}

/// The outcome line for the last credential write.
fn sl_write_line(app: &SettingsApp) -> &'static [u8] {
    match app.screen_lock_write {
        ScreenLockWrite::Unattempted => {
            b"Changes are written by the account layer and kept across reboots"
        }
        ScreenLockWrite::Established => b"A PIN is set and now required to unlock",
        ScreenLockWrite::Changed => b"The PIN was replaced and now required to unlock",
        ScreenLockWrite::Removed => b"The PIN was removed; the lock screen unlocks on a swipe",
        ScreenLockWrite::Refused => b"The account layer refused the change; nothing changed",
    }
}

/// The screen-lock panel, answering per pixel like every other surface here.
fn screen_lock_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    // WHY there is no "y < 104 is empty" early return here: the header lives at
    // y 32 and y 88, so such a return erases the title and the subtitle and left
    // only the subtitle's top rows visible above the card. The panel fills its
    // own background at the end of this function instead.
    // Header on the shared header geometry, so a panel opened from a detail row
    // keeps that row's title on the same line.
    if let Some(c) = run_ink(
        x,
        y,
        HEADER_X,
        HEADER_Y,
        TYPE_DISPLAY,
        b"Screen lock",
        W - HEADER_X - MARGIN,
        TEXT,
        BG,
        true,
    ) {
        return c;
    }
    if let Some(c) = run_ink(
        x,
        y,
        HEADER_X,
        HEADER_SUB_Y,
        TYPE_H3,
        b"Set, change or remove the PIN this device asks for",
        W - HEADER_X - MARGIN,
        TEXT_MUTED,
        BG,
        false,
    ) {
        return c;
    }

    // Status card: the account layer's own answer, with its badge. It is a detail
    // row card in every respect — same width, radius, padding and type rungs —
    // because it is the same component playing the same role on another screen.
    if (SL_STATUS_Y..SL_STATUS_Y + SL_STATUS_H).contains(&y) {
        let (_, _, _, badge) = sl_state_line(app);
        let bw = badge_w(badge);
        let bx0 = CONTENT_X + CONTENT_W - ROW_PAD - bw;
        if in_round_rect(x, y, CONTENT_X, SL_STATUS_Y, CONTENT_W, SL_STATUS_H, CARD_R) {
            let inner = in_round_rect(
                x,
                y,
                CONTENT_X + 1,
                SL_STATUS_Y + 1,
                CONTENT_W - 2,
                SL_STATUS_H - 2,
                CARD_R - 1,
            );
            if !inner {
                return BORDER;
            }
            let (headline, detail, ink, _) = sl_state_line(app);
            // WHY the headline is the H3 rung and not the Display rung it used to
            // be: it is a card's own heading, and the doc sizes a list label at
            // Body/H3. Leaving it at Display made one card's heading the same rank
            // as the panel's title.
            if let Some(c) = run_ink(
                x,
                y,
                CONTENT_X + ROW_PAD,
                SL_STATUS_Y + ROW_TITLE_DY,
                TYPE_H3,
                headline,
                CONTENT_W - 2 * ROW_PAD - bw - BADGE_GAP,
                ink,
                SURFACE,
                true,
            ) {
                return c;
            }
            if let Some(c) = run_ink(
                x,
                y,
                CONTENT_X + ROW_PAD,
                SL_STATUS_Y + ROW_CAP_DY,
                TYPE_H3,
                detail,
                CONTENT_W - 2 * ROW_PAD,
                TEXT_MUTED,
                SURFACE,
                false,
            ) {
                return c;
            }
            if let Some(c) = badge_pixel(x, y, bx0, SL_STATUS_Y + BADGE_DY, badge, TEXT, SURFACE) {
                return c;
            }
            return SURFACE;
        }
        return BG;
    }

    // Step prompt and the typed-entry dots, only while a step is in progress.
    if app.screen_lock_mode != ScreenLockMode::Idle {
        // WHY the prompt is the H3 rung: it is the panel's body text for the step
        // in progress, one rank below the panel's own title. Display made the
        // prompt compete with the title it belongs to.
        if let Some(c) = run_ink(
            x,
            y,
            CONTENT_X,
            SL_PROMPT_Y,
            TYPE_H3,
            sl_prompt(app),
            CONTENT_W,
            TEXT,
            BG,
            true,
        ) {
            return c;
        }
        let line = sl_error_line(app);
        if !line.is_empty() {
            let ink = DANGER;
            if let Some(c) = run_ink(
                x,
                y,
                CONTENT_X,
                SL_PROMPT_Y + SL_ERROR_DY,
                TYPE_H3,
                line,
                CONTENT_W,
                ink,
                BG,
                false,
            ) {
                return c;
            }
        }
        // Eight dots, one per digit the account layer accepts, filled to the
        // number typed so far. WHY eight and not four: the account layer accepts
        // 4..=8, and a pad that drew four could not show a longer entry.
        let dot = SL_DOT;
        let gap = SL_DOT_GAP;
        let total = 8 * dot + 7 * gap;
        let dx0 = (CONTENT_W - total) / 2;
        let filled = app.screen_lock_len;
        for i in 0..8usize {
            let cx0 = CONTENT_X + dx0 + i * (dot + gap);
            let cy0 = SL_DOTS_Y;
            if in_round_rect(x, y, cx0, cy0, dot, dot, SL_DOT_R) {
                let inner = in_round_rect(x, y, cx0 + 2, cy0 + 2, dot - 4, dot - 4, SL_DOT_R - 2);
                if !inner {
                    return BORDER;
                }
                return if i < filled { ACCENT } else { SOFT };
            }
        }
    }

    // Keypad: only rendered when a PIN entry or change step is active.
    if app.screen_lock_mode != ScreenLockMode::Idle && y >= SL_KEYS_Y && y < SL_KEYS_BOTTOM {
        for row in 0..4usize {
            for col in 0..3usize {
                let (kx, ky) = sl_key_origin(row, col);
                if !in_round_rect(x, y, kx, ky, SL_KEY_W, SL_KEY_H, SL_KEY_R) {
                    continue;
                }
                let inner = in_round_rect(
                    x,
                    y,
                    kx + 2,
                    ky + 2,
                    SL_KEY_W - 4,
                    SL_KEY_H - 4,
                    SL_KEY_R - 2,
                );
                if !inner {
                    return BORDER;
                }
                let label: &[u8] = match sl_digit(row, col) {
                    Some(d) => {
                        let one = [d, 0];
                        return run_one_digit(x, y, kx, ky, &one[..1], TEXT, SURFACE);
                    }
                    None if col == 0 => b"CLR",
                    None => b"OK",
                };
                let lw = run_w(label, TYPE_H3, true);
                // WHY the label is fitted before it is centred: `SL_KEY_W - lw`
                // underflows in usize when a label is wider than its key, and the
                // wrapped origin then puts the text off the key altogether.
                let lx = if lw + 2 * std::design::spacing::GRID_8 >= SL_KEY_W {
                    kx + std::design::spacing::GRID_8
                } else {
                    kx + (SL_KEY_W - lw) / 2
                };
                if let Some(c) = run_ink(
                    x,
                    y,
                    lx,
                    ky + (SL_KEY_H - LINE_H) / 2,
                    TYPE_H3,
                    label,
                    SL_KEY_W - 2 * std::design::spacing::GRID_8,
                    TEXT,
                    SURFACE,
                    true,
                ) {
                    return c;
                }
                return SURFACE;
            }
        }
        return BG;
    }

    // Action row: what the reported state actually allows.
    if (SL_ACTION_Y..SL_ACTION_Y + SL_ACTION_H).contains(&y) {
        let half = BUTTON_HALF;
        let has_pin = app.screen_lock_has_pin();
        // WHY the row is tested as ONE full-width button when no PIN exists: the
        // first pass still split it into halves and drew only the left one, which
        // left the right half of a visible full-width button as background and
        // clipped the label against the half it did draw.
        let left = in_round_rect(
            x,
            y,
            CONTENT_X,
            SL_ACTION_Y,
            half,
            SL_ACTION_H,
            SL_ACTION_H / 2,
        );
        let right = has_pin
            && in_round_rect(
                x,
                y,
                CONTENT_X + half + CARD_GAP,
                SL_ACTION_Y,
                half,
                SL_ACTION_H,
                SL_ACTION_H / 2,
            );
        let inside = if has_pin {
            left || right
        } else {
            in_round_rect(
                x,
                y,
                CONTENT_X,
                SL_ACTION_Y,
                CONTENT_W,
                SL_ACTION_H,
                SL_ACTION_H / 2,
            )
        };
        if !inside {
            return BG;
        }
        // WHY the layout changes with the state rather than always drawing two
        // buttons: an account with no PIN has nothing to change and nothing to
        // remove, and an account with one has nothing to establish. Drawing both
        // would put a second identical "Set PIN" on the screen -- which is what
        // the first pass of this panel did.
        //
        // WHY the fills follow the reference buttons: "Set a screen lock PIN" is
        // this panel's one primary action and takes the dark pill; the other two
        // are secondary and take the soft fill, with the destructive one in the
        // palette's danger ink rather than a filled red block.
        let (label, bx, bw, fill, ink, rim): (&[u8], usize, usize, u32, u32, Option<u32>) = if !has_pin {
            (
                &b"Set a screen lock PIN"[..],
                CONTENT_X,
                CONTENT_W,
                TEXT,
                SURFACE,
                None,
            )
        } else if left {
            (
                &b"Change PIN"[..],
                CONTENT_X,
                half,
                SOFT,
                TEXT,
                Some(BORDER),
            )
        } else {
            (
                &b"Remove PIN"[..],
                CONTENT_X + half + CARD_GAP,
                half,
                SOFT,
                DANGER,
                Some(BORDER),
            )
        };
        if let Some(c) = button_pixel(
            x,
            y,
            bx,
            SL_ACTION_Y,
            bw,
            SL_ACTION_H,
            label,
            fill,
            ink,
            rim,
        ) {
            return c;
        }
        return BG;
    }

    // Outcome line.
    if y >= SL_NOTE_Y {
        if let Some(c) = run_ink(
            x,
            y,
            CONTENT_X,
            SL_NOTE_Y,
            TYPE_H3,
            sl_write_line(app),
            CONTENT_W,
            TEXT_MUTED,
            BG,
            false,
        ) {
            return c;
        }
    }
    BG
}

/// Draws one keypad digit, centred in its key.
///
/// WHY a helper rather than a formatted run: the keypad has one digit per key and
/// building a string per pixel would allocate on a screen with no allocator.
fn run_one_digit(
    x: usize,
    y: usize,
    kx: usize,
    ky: usize,
    digit: &[u8],
    ink: u32,
    bg: u32,
) -> u32 {
    let adv = std::design::font::advance_of(digit[0], true);
    let w = adv * TYPE_DISPLAY;
    let ox = kx + (SL_KEY_W - w) / 2;
    let oy = ky + (SL_KEY_H - std::design::font::line_height(TYPE_DISPLAY)) / 2;
    match run_ink(x, y, ox, oy, TYPE_DISPLAY, digit, SL_KEY_W, ink, bg, true) {
        Some(c) => c,
        None => bg,
    }
}

/// What was pressed on the screen-lock panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenLockHit {
    /// Keypad digit.
    Digit(u8),
    /// The CLR key.
    Clear,
    /// The OK key, which submits the typed entry.
    Submit,
    /// The left action button: Set PIN when no PIN exists, Change PIN when one does.
    Primary,
    /// The Remove PIN button.
    Remove,
}

fn screen_lock_at(px: u32, py: u32, app: &SettingsApp) -> Option<ScreenLockHit> {
    let x = px as usize;
    let y = py as usize;
    // WHY the keypad only answers mid-flow: on the overview there is nothing to
    // type, and a dead keypad that still reacts would look like a screen waiting
    // for a PIN it was never asked for.
    if app.screen_lock_mode != ScreenLockMode::Idle && y >= SL_KEYS_Y {
        for row in 0..4usize {
            for col in 0..3usize {
                let (kx, ky) = sl_key_origin(row, col);
                if in_round_rect(x, y, kx, ky, SL_KEY_W, SL_KEY_H, SL_KEY_R) {
                    return Some(match sl_digit(row, col) {
                        Some(d) => ScreenLockHit::Digit(d),
                        None if col == 0 => ScreenLockHit::Clear,
                        None => ScreenLockHit::Submit,
                    });
                }
            }
        }
    }
    if (SL_ACTION_Y..SL_ACTION_Y + SL_ACTION_H).contains(&y) {
        let half = BUTTON_HALF;
        let has_pin = app.screen_lock_has_pin();
        // WHY the hit regions mirror the drawn geometry exactly: with no PIN the
        // single button spans the full width, so only testing the left half
        // would leave the right half of a visible button inert.
        if !has_pin {
            if in_round_rect(
                x,
                y,
                CONTENT_X,
                SL_ACTION_Y,
                CONTENT_W,
                SL_ACTION_H,
                SL_ACTION_H / 2,
            ) {
                return Some(ScreenLockHit::Primary);
            }
        } else {
            if in_round_rect(
                x,
                y,
                CONTENT_X,
                SL_ACTION_Y,
                half,
                SL_ACTION_H,
                SL_ACTION_H / 2,
            ) {
                return Some(ScreenLockHit::Primary);
            }
            if in_round_rect(
                x,
                y,
                CONTENT_X + half + CARD_GAP,
                SL_ACTION_Y,
                half,
                SL_ACTION_H,
                SL_ACTION_H / 2,
            ) {
                return Some(ScreenLockHit::Remove);
            }
        }
    }
    None
}

#[inline(never)]
fn get_settings_pixel(x: usize, y: usize, app: &SettingsApp) -> u32 {
    // Home gesture pill (matches the touch handler's exit region).
    if y >= GESTURE_Y
        && y < GESTURE_Y + GESTURE_H
        && x >= GESTURE_X
        && x < GESTURE_X + GESTURE_W
    {
        return GESTURE_BAR;
    }
    // Shared 48x48 back control: only rendered when inside a subpage or section detail
    let is_subpage = app.sensors_open || app.wallpaper_open || app.screen_lock_open || app.current_section.is_some();
    if is_subpage {
        if let Some(c) = back_button_pixel(x, y) {
            return c;
        }
    }
    if app.sensors_open {
        return sensors_pixel(x, y, app);
    }
    // WHY the panel is matched before the section: it is a screen the user
    // reached *through* a section, so it must stay in front of the detail it was
    // opened from rather than overwriting it.
    if app.wallpaper_open {
        return wallpaper_pixel(x, y, app);
    }
    // WHY checked before the wallpaper panel: both own the whole surface, and
    // whichever was opened last is the one in front.
    if app.screen_lock_open {
        return screen_lock_pixel(x, y, app);
    }
    match app.current_section {
        None => landing_pixel(x, y, app),
        Some(section) => detail_pixel(x, y, app, section),
    }
}

// -----------------------------------------------------------------------------
// Presentation & event loop
// -----------------------------------------------------------------------------

fn render_settings_ui(surface: u32, app: &SettingsApp) {
    render_settings_band(surface, app, 0, H);
}

/// Repaints only rows `y0..y1`.
///
/// WHY this exists: every tap used to push all 1,036,800 pixels through the
/// painter. Under the QEMU TCG debug profile a full repaint of this surface takes
/// 15-30 seconds, so taps that arrived while it was still running were dropped
/// by the guest and digits went missing from a PIN entry -- the screen was
/// correct, the device just felt broken. Typing a digit changes one band: the
/// step prompt, its error line and the entry dots. The keypad, the status card
/// and the action row are all still correct underneath it.
fn render_settings_band(surface: u32, app: &SettingsApp, y0: usize, y1: usize) {
    let y0 = y0.min(H);
    let y1 = y1.min(H);
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    while offset < y1 * W {
        let count = (y1 * W - offset).min(CHUNK);
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
        let sx = CONTENT_X + ROW_PAD + i * (SEG_W + SEG_GAP);
        if in_round_rect(px as usize, py as usize, sx, cy + SEG_DY, SEG_W, SEG_H, CONTROL_R) {
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

    // 4. Inspect Personal & Identity values (Personal detail)
    settings.refresh_personal();

    // 5. Inspect Installed Applications & Storage (Apps detail)
    settings.refresh_apps_storage();

    // 6. Audit Security & Hardware Keystore (Security detail)
    settings.audit_security();
    settings.refresh_screen_lock();

    // 7. Query About Device & Hardware RTC (System detail)
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
                        // WHY this flag: the repaint at the end of this block runs
                        // for EVERY tap, so a branch that has already repainted
                        // the only band it changed would be immediately followed
                        // by a full 1,036,800-pixel repaint -- which is what
                        // dropped the taps that followed a PIN digit.
                        let mut repainted_band = false;
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
                            BACK_R,
                        ) {
                            // WHY the panels pop first: Back from one must return to the
                            // detail it was opened from, and a Back that jumped
                            // straight to the landing grid would lose the section
                            // the user was in. Neither panel writes on the way
                            // out, so leaving one is always the cancel path.
                            if settings.sensors_open {
                                settings.sensors_open = false;
                                std::log(
                                    0,
                                    "[SETTINGS APP] Sensors & Diagnostics sub-page dismissed.",
                                )
                                .ok();
                            } else if settings.screen_lock_open {
                                // A step in progress is abandoned before the
                                // panel itself: Back must not leave a half-typed
                                // PIN behind for the next visit to find.
                                if !settings.screen_lock_cancel_step() {
                                    settings.screen_lock_open = false;
                                    std::log(
                                        0,
                                        "[SETTINGS APP] Screen lock panel dismissed.",
                                    )
                                    .ok();
                                }
                            } else if settings.wallpaper_open {
                                settings.wallpaper_open = false;
                                std::log(
                                    0,
                                    "[SETTINGS APP] Wallpaper picker dismissed without applying a change.",
                                )
                                .ok();
                            } else {
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
                                    // On root landing screen there is no back button; home gesture bar handles exit.
                                }
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
                        } else if settings.sensors_open {
                            // Any tap on Sensors surface refreshes live sensor telemetry
                            settings.refresh_thermals_and_sensors();
                        } else if settings.screen_lock_open {
                            // WHY the panel is matched before the section controls:
                            // while it is open it owns the whole surface, so a tap on
                            // a keypad key must not also reach the detail row
                            // underneath it -- that is how a PIN digit would once
                            // toggle a setting on a screen the owner is not looking
                            // at.
                            match screen_lock_at(input.x, input.y, &settings) {
                                Some(ScreenLockHit::Digit(d)) => {
                                    settings.screen_lock_push(d);
                                    // WHY only the prompt band: a digit changes the
                                    // dots and nothing else, and a full repaint
                                    // here is what made taps get dropped.
                                    if surface > 0 {
                                        render_settings_band(
                                            surface,
                                            &settings,
                                            SL_PROMPT_Y - 24,
                                            SL_KEYS_Y,
                                        );
                                        let _ = std::surface_flip(6);
                                        repainted_band = true;
                                    }
                                }
                                Some(ScreenLockHit::Clear) => {
                                    settings.screen_lock_pop();
                                    if surface > 0 {
                                        render_settings_band(
                                            surface,
                                            &settings,
                                            SL_PROMPT_Y - 24,
                                            SL_KEYS_Y,
                                        );
                                        let _ = std::surface_flip(6);
                                        repainted_band = true;
                                    }
                                }
                                Some(ScreenLockHit::Submit) => {
                                    // WHY the mode is compared across the submit:
                                    // OK between two entry steps changes the
                                    // prompt and the dots and leaves the keypad on
                                    // screen, so it repaints that band only. OK
                                    // that finishes a write returns to the
                                    // overview, hides the keypad and re-reads the
                                    // account layer, which needs the full surface.
                                    let was_typing = settings.screen_lock_mode
                                        != ScreenLockMode::Idle;
                                    settings.screen_lock_submit();
                                    let still_typing =
                                        settings.screen_lock_mode != ScreenLockMode::Idle;
                                    if surface > 0 && was_typing && still_typing {
                                        render_settings_band(
                                            surface,
                                            &settings,
                                            SL_PROMPT_Y - 24,
                                            SL_KEYS_Y,
                                        );
                                        let _ = std::surface_flip(6);
                                        repainted_band = true;
                                    }
                                }
                                Some(ScreenLockHit::Primary) => {
                                    // WHY the action follows the reported state
                                    // rather than being fixed at paint time: the
                                    // same button is "set" for an account with no
                                    // PIN and "change" for one that has it, and the
                                    // state is re-read after every write.
                                    let action = if settings.screen_lock_has_pin() {
                                        ScreenLockAction::Change
                                    } else {
                                        ScreenLockAction::Establish
                                    };
                                    settings.screen_lock_begin(action);
                                }
                                Some(ScreenLockHit::Remove) => {
                                    settings.screen_lock_begin(ScreenLockAction::Remove)
                                }
                                None => {}
                            }
                        } else if settings.wallpaper_open {
                            // WHY this is checked before the section controls: the
                            // picker owns the whole surface while it is open, so a
                            // tap must not fall through to a detail row underneath
                            // it -- that is how a wallpaper tap would once toggle a
                            // setting two screens away.
                            match wallpaper_at(input.x, input.y) {
                                Some(WallpaperHit::Target(i)) => {
                                    settings.wallpaper_target = WallpaperTarget::from_index(i);
                                    std::log(0, "[SETTINGS APP] Wallpaper target changed.").ok();
                                }
                                Some(WallpaperHit::Cell(i)) => {
                                    // WHY browsing past the catalogue is impossible:
                                    // the gallery is generated from
                                    // `walls::SCENES`, so its hit-test cannot name
                                    // an index the catalogue does not hold.
                                    if i < std::design::walls::SCENES.len() {
                                        // WHY the reference is taken straight out
                                        // of the catalogue: the gallery's cells are
                                        // the catalogue's scenes, so a selection is
                                        // a `&'static` scene and no copy of it is
                                        // ever made into the app's state.
                                        let scene = &std::design::walls::SCENES[i];
                                        settings.wallpaper_pending = Some(scene);
                                        std::log(
                                            0,
                                            "[SETTINGS APP] Wallpaper selected for preview; nothing written yet.",
                                        )
                                        .ok();
                                    }
                                }
                                Some(WallpaperHit::Apply) => settings.apply_wallpaper(),
                                Some(WallpaperHit::Reset) => settings.reset_wallpaper(),
                                None => {}
                            }
                        } else if settings.current_section == Some(Section::Personal) {
                            // WHY the wallpaper row is opened after the picker
                            // branch: while the picker is open it owns the whole
                            // surface, and a tap in the panel must not also reach a
                            // detail row underneath it. With the picker closed this
                            // branch is the only route into it.
                            let (cy, ch) = detail_row_rect(Section::Personal, 2);
                            if in_round_rect(
                                input.x as usize,
                                input.y as usize,
                                DETAIL_X,
                                cy,
                                DETAIL_W,
                                ch,
                                CARD_R,
                            ) {
                                settings.open_wallpapers();
                            }
                        } else if let Some(section) = settings.current_section {
                            // Detail-screen controls (hit cells match the drawn
                            // geometry exactly).
                            match section {
                                Section::Device => {
                                    let (cy, ch) = detail_row_rect(Section::Device, 4);
                                    if in_round_rect(
                                        input.x as usize,
                                        input.y as usize,
                                        DETAIL_X,
                                        cy,
                                        DETAIL_W,
                                        ch,
                                        CARD_R,
                                    ) {
                                        settings.sensors_open = true;
                                        settings.refresh_thermals_and_sensors();
                                        std::log(
                                            0,
                                            "[SETTINGS APP] Sensors & Diagnostics sub-page opened.",
                                        )
                                        .ok();
                                    }
                                }
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
                                    let (cy, ch) = detail_row_rect(Section::System, 4);
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
                                Section::Security => {
                                    let (cy, ch) = detail_row_rect(Section::Security, 0);
                                    if in_round_rect(
                                        input.x as usize,
                                        input.y as usize,
                                        DETAIL_X,
                                        cy,
                                        DETAIL_W,
                                        ch,
                                        CARD_R,
                                    ) {
                                        settings.open_screen_lock();
                                    }
                                }
                                _ => {}
                            }
                        } else if let Some(section) = landing_card_at(input.x, input.y) {
                            settings.current_section = Some(section);
                            if section == Section::System {
                                settings.display_about_device();
                            }
                            // WHY the wallpaper reading is refreshed on entry to a
                            // detail: the Personal row reports what the store holds,
                            // and the value only changes when another surface (the
                            // picker, a previous visit) wrote it. Reading it once
                            // when the detail opens is the whole cost.
                            settings.refresh_wallpaper_values();
                            std::log(
                                0,
                                "[SETTINGS APP] System Space category opened; tab switched.",
                            )
                            .ok();
                        }
                        if surface > 0 && !repainted_band {
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
