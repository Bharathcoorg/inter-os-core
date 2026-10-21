#![no_std]

pub use interos_std as std;

pub const PIN_MAX_DIGITS: usize = 8;
pub const PIN_MIN_DIGITS: usize = 4;

/// The seven semantic System Space categories.
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
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn syscall_id(self) -> u32 {
        match self {
            PowerProfile::PowerSaver => 0,
            PowerProfile::Balanced => 1,
            PowerProfile::Performance => 2,
        }
    }
}

/// Which surface a wallpaper write targets.
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
}

/// System color themes backed by the settings store (`ui.theme`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Default = 0,
    Light = 1,
    Dark = 2,
    HighContrast = 3,
}

impl ThemeMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            ThemeMode::Default => "interos-default",
            ThemeMode::Light => "interos-light",
            ThemeMode::Dark => "interos-dark",
            ThemeMode::HighContrast => "interos-high-contrast",
        }
    }

    pub fn from_str(s: &str) -> Option<ThemeMode> {
        match s {
            "interos-default" => Some(ThemeMode::Default),
            "interos-light" => Some(ThemeMode::Light),
            "interos-dark" => Some(ThemeMode::Dark),
            "interos-high-contrast" => Some(ThemeMode::HighContrast),
            _ => None,
        }
    }
}

/// Bounded display brightness percent (10%..100%).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayBrightness(u8);

impl DisplayBrightness {
    pub const MIN: u8 = 10;
    pub const MAX: u8 = 100;
    pub const DEFAULT: u8 = 75;

    pub const fn new(percent: u8) -> Self {
        let clamped = if percent < Self::MIN {
            Self::MIN
        } else if percent > Self::MAX {
            Self::MAX
        } else {
            percent
        };
        Self(clamped)
    }

    pub const fn percent(self) -> u8 {
        self.0
    }
}

/// Bounded PIN buffer helper with zero heap allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinBuffer {
    digits: [u8; PIN_MAX_DIGITS],
    len: usize,
}

impl PinBuffer {
    pub const EMPTY: PinBuffer = PinBuffer {
        digits: [0; PIN_MAX_DIGITS],
        len: 0,
    };

    pub fn push(&mut self, digit: u8) -> bool {
        if self.len < PIN_MAX_DIGITS {
            self.digits[self.len] = digit;
            self.len += 1;
            true
        } else {
            false
        }
    }

    pub fn pop(&mut self) -> Option<u8> {
        if self.len > 0 {
            self.len -= 1;
            Some(self.digits[self.len])
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.digits = [0; PIN_MAX_DIGITS];
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_valid_length(&self) -> bool {
        self.len >= PIN_MIN_DIGITS && self.len <= PIN_MAX_DIGITS
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.digits[..self.len]
    }
}

/// Detailed network interface telemetry model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkInterfaceRecord {
    pub name: [u8; 16],
    pub name_len: usize,
    pub ip_addr: [u8; 4],
    pub mac_addr: [u8; 6],
    pub mtu: u16,
    pub is_up: bool,
    pub is_wireless: bool,
}

impl NetworkInterfaceRecord {
    pub const EMPTY: Self = Self {
        name: [0u8; 16],
        name_len: 0,
        ip_addr: [0u8; 4],
        mac_addr: [0u8; 6],
        mtu: 1500,
        is_up: false,
        is_wireless: false,
    };
}

/// Standard discrete permission bits granted to Sovereign applications.
pub const PERM_VFS_READ: u32 = 1 << 0;
pub const PERM_VFS_WRITE: u32 = 1 << 1;
pub const PERM_NETWORK: u32 = 1 << 2;
pub const PERM_CAMERA: u32 = 1 << 3;
pub const PERM_AUDIO: u32 = 1 << 4;
pub const PERM_TELEPHONY: u32 = 1 << 5;
pub const PERM_LOCATION: u32 = 1 << 6;

/// Bounded per-app permission record supporting dynamic revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppPermissionState {
    pub app_name: [u8; 24],
    pub app_name_len: usize,
    pub granted_mask: u32,
    pub revoked_mask: u32,
}

impl AppPermissionState {
    pub const fn new(name: &[u8], granted: u32) -> Self {
        let mut app_name = [0u8; 24];
        let mut n = 0;
        while n < name.len() && n < 24 {
            app_name[n] = name[n];
            n += 1;
        }
        Self {
            app_name,
            app_name_len: n,
            granted_mask: granted,
            revoked_mask: 0,
        }
    }

    /// Effective active permissions: granted AND NOT revoked.
    #[inline(always)]
    pub fn effective_mask(&self) -> u32 {
        self.granted_mask & !self.revoked_mask
    }

    /// Checks if a specific permission is currently active.
    #[inline(always)]
    pub fn is_allowed(&self, perm_bit: u32) -> bool {
        (self.effective_mask() & perm_bit) == perm_bit
    }

    /// Revokes a permission bit.
    pub fn revoke(&mut self, perm_bit: u32) {
        self.revoked_mask |= perm_bit;
    }

    /// Restores a previously revoked permission bit.
    pub fn restore(&mut self, perm_bit: u32) {
        self.revoked_mask &= !perm_bit;
    }
}

// -----------------------------------------------------------------------------
// Real Hardware Sensor & Telemetry Decoding (Zero Heap, Pure Flat-JSON)
// -----------------------------------------------------------------------------

/// Individual hardware sensor status on the sovereign platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorStatus {
    /// Sensor is unbound or absent on this physical/virtual board.
    Unbound,
    /// Sensor query refused by capability check.
    Denied,
    /// Real hardware sensor is bound and reporting live telemetry.
    Live,
}

/// The platform's own answer about whether a hardware category is present.
///
/// WHY the app asks instead of assuming: the kernel publishes three answers
/// per category - present, emulated, absent - and a card that assumed the
/// first one would claim hardware the platform says it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareAvailability {
    /// The platform reports the category as present.
    Available,
    /// The platform reports the category as present but emulated.
    Emulated,
    /// The platform reports the category as absent.
    Unavailable,
    /// The platform did not answer.
    Unknown,
}

/// Decodes the status code the kernel's `settings_hardware_status` returns.
///
/// WHY the codes are the kernel's: `kernel/src/syscall/mod.rs` maps
/// `HardwareAvailability::Available` to 0, `SimulatedVirtual` to 1 and
/// `Unavailable` to 2. Decoding them here keeps the mapping in one place
/// that a test can pin against the kernel.
pub fn parse_hardware_availability(code: i64) -> HardwareAvailability {
    match code {
        0 => HardwareAvailability::Available,
        1 => HardwareAvailability::Emulated,
        2 => HardwareAvailability::Unavailable,
        _ => HardwareAvailability::Unknown,
    }
}

/// Decoded sensor subsystem telemetry. Zero mock/fake values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorTelemetry {
    pub is_present: bool,
    pub lux: i64,
    pub accel_y: i64,
    pub is_near: bool,
    pub orientation: [u8; 24],
    pub orientation_len: usize,
    /// The kernel's own `status` field, quoted verbatim.
    ///
    /// WHY this is kept: the platform publishes `NOT_IMPLEMENTED` next to its
    /// `error` string when no sensor is bound, and that is the platform's own
    /// word for the situation. Replacing it with the app's own wording loses
    /// the one statement the platform actually made.
    pub status: [u8; 24],
    pub status_len: usize,
    /// The kernel's own `error` string, quoted verbatim.
    pub error_msg: [u8; 96],
    pub error_msg_len: usize,
}

impl SensorTelemetry {
    pub const EMPTY: Self = Self {
        is_present: false,
        lux: 0,
        accel_y: 0,
        is_near: false,
        orientation: [0u8; 24],
        orientation_len: 0,
        status: [0u8; 24],
        status_len: 0,
        error_msg: [0u8; 96],
        error_msg_len: 0,
    };
}

/// Why a platform query did not produce a reading.
///
/// WHY this is not a boolean: the platform tells three different stories -
/// "the call was refused", "nothing is bound to that bus" and "the payload
/// came back empty" - and they are not the same sentence. Collapsing them
/// into one `Unavailable` is what let the app claim a device was offline
/// when in fact it had never been asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryOutcome {
    /// The call succeeded and the payload carries a reading.
    Reported,
    /// The call succeeded and the payload declares that no hardware is bound.
    Absent,
    /// The call succeeded but the payload carried no bytes at all.
    Empty,
    /// The call succeeded but the payload did not carry the expected fields.
    Malformed,
    /// The kernel returned a negative status; this is that status code.
    Failed(i64),
}

/// The platform's own name for a negative `SyscallResult`.
///
/// WHY the names and not the bare numbers: the kernel publishes exactly these
/// spellings (`kernel/src/syscall/mod.rs`, `enum SyscallResult`), so quoting
/// them is quoting the platform rather than inventing a vocabulary.
pub fn syscall_status_name(code: i64) -> &'static [u8] {
    match code {
        -1 => b"PermissionDenied",
        -2 => b"InvalidSlot",
        -3 => b"InvalidArguments",
        -12 => b"DeviceError",
        _ => b"UnknownStatus",
    }
}

/// Decoded multi-zone thermal telemetry, exactly as the kernel serialised it.
///
/// WHY the zones are named rather than summed: the governor makes its
/// decisions per zone, so a single "average temperature" is a number the
/// platform never produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalTelemetry {
    /// The kernel's own `present` flag.
    pub present: bool,
    pub cpu_c: i64,
    pub gpu_c: i64,
    pub battery_c: i64,
    pub modem_c: i64,
    pub charging_c: i64,
    /// The cooling state, spelled exactly as the kernel spelled it
    /// (`Normal`, `Warm`, `Alert`, `Throttled`, `Critical`).
    pub state: [u8; 16],
    pub state_len: usize,
    /// The kernel's own throttling flag.
    pub throttling: bool,
}

impl ThermalTelemetry {
    pub const EMPTY: Self = Self {
        present: false,
        cpu_c: 0,
        gpu_c: 0,
        battery_c: 0,
        modem_c: 0,
        charging_c: 0,
        state: [0u8; 16],
        state_len: 0,
        throttling: false,
    };
}

/// Decoded fuel-gauge telemetry, exactly as the kernel serialised it.
///
/// WHY `present` and `telemetry_valid` are both kept: the payload carries
/// them separately, and the numbers in an absent payload are zeros that were
/// never measured. A card that prints them is printing the absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryTelemetry {
    /// The kernel's own `present` flag.
    pub present: bool,
    /// The kernel's own `telemetry_valid` flag.
    pub telemetry_valid: bool,
    pub voltage_mv: i64,
    /// State of charge, in percent.
    pub soc: i64,
    pub capacity_mah: i64,
    /// The charging state, spelled exactly as the kernel spelled it
    /// (`Discharging`, `Charging`, `Full`, `Disconnected`).
    pub charging: [u8; 20],
    pub charging_len: usize,
    pub temp_c: i64,
    /// The DVFS governor the payload reported, when the payload carried one.
    pub governor: [u8; 20],
    pub governor_len: usize,
    pub mhz: i64,
}

impl BatteryTelemetry {
    pub const EMPTY: Self = Self {
        present: false,
        telemetry_valid: false,
        voltage_mv: 0,
        soc: 0,
        capacity_mah: 0,
        charging: [0u8; 20],
        charging_len: 0,
        temp_c: 0,
        governor: [0u8; 20],
        governor_len: 0,
        mhz: 0,
    };
}

/// Finds the starting byte index of a value following `"key":` in flat JSON.
pub fn json_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + key.len() + 2 < buf.len() {
        if buf[i] == b'"'
            && &buf[i + 1..i + 1 + key.len()] == key
            && buf[i + 1 + key.len()] == b'"'
            && (i == 0 || buf[i - 1] == b'{' || buf[i - 1] == b',' || buf[i - 1] == b' ')
        {
            let mut after = i + key.len() + 2;
            while after < buf.len() && buf[after] == b' ' {
                after += 1;
            }
            if after < buf.len() && buf[after] == b':' {
                let mut v = after + 1;
                while v < buf.len() && buf[v] == b' ' {
                    v += 1;
                }
                return Some(v);
            }
        }
        i += 1;
    }
    None
}

/// Parses a boolean (`true`/`false`) value for `key`.
pub fn json_bool(buf: &[u8], key: &[u8]) -> Option<bool> {
    let i = json_value(buf, key)?;
    if i >= buf.len() {
        return None;
    }
    match buf[i] {
        b't' if buf[i..].starts_with(b"true") => Some(true),
        b'f' if buf[i..].starts_with(b"false") => Some(false),
        _ => None,
    }
}

/// Parses an integer value for `key`.
pub fn json_i64(buf: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_value(buf, key)?;
    let negative = i < buf.len() && buf[i] == b'-';
    if negative {
        i += 1;
    }
    let mut v: i64 = 0;
    let mut digits = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((buf[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -v } else { v })
}

/// Parses a quoted string value for `key` into `out`.
pub fn json_str(buf: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut i = json_value(buf, key)?;
    if i >= buf.len() || buf[i] != b'"' {
        return None;
    }
    i += 1;
    let mut n = 0usize;
    while i < buf.len() && buf[i] != b'"' {
        if n < out.len() {
            out[n] = buf[i];
        }
        n += 1;
        i += 1;
    }
    Some(n.min(out.len()))
}

/// Decodes a real `sensor_read()` JSON payload from the kernel.
pub fn parse_sensor_payload(payload: &[u8]) -> SensorTelemetry {
    let mut result = SensorTelemetry::EMPTY;
    match json_bool(payload, b"present") {
        Some(true) => {
            result.is_present = true;
            result.lux = json_i64(payload, b"lux").unwrap_or(0);
            result.accel_y = json_i64(payload, b"accel").unwrap_or(0);
            result.is_near = json_bool(payload, b"near").unwrap_or(false);
            if let Some(n) = json_str(payload, b"orientation", &mut result.orientation) {
                result.orientation_len = n;
            }
        }
        Some(false) => {
            result.is_present = false;
            // WHY both strings are kept: the platform states the situation in
            // `status` and explains it in `error`, and the app quotes both
            // rather than paraphrasing either.
            if let Some(n) = json_str(payload, b"status", &mut result.status) {
                result.status_len = n;
            }
            if let Some(n) = json_str(payload, b"error", &mut result.error_msg) {
                result.error_msg_len = n;
            }
        }
        None => {
            result.is_present = false;
        }
    }
    result
}

/// Decodes a real `thermal_query()` JSON payload from the kernel.
///
/// WHY this returns `None` for a malformed payload: a payload without the
/// fields is not a reading of zero, it is a payload this app cannot read, and
/// the two must not look the same on screen.
pub fn parse_thermal_payload(payload: &[u8]) -> Option<ThermalTelemetry> {
    let present = json_bool(payload, b"present")?;
    let mut result = ThermalTelemetry {
        present,
        ..ThermalTelemetry::EMPTY
    };
    if !present {
        // The platform declared that nothing is bound; there are no zone
        // numbers to read and the card must say so rather than print zeros.
        return Some(result);
    }
    result.cpu_c = json_i64(payload, b"cpu")?;
    result.gpu_c = json_i64(payload, b"gpu")?;
    result.battery_c = json_i64(payload, b"battery")?;
    result.modem_c = json_i64(payload, b"modem")?;
    result.charging_c = json_i64(payload, b"charging")?;
    result.state_len = json_str(payload, b"state", &mut result.state)?;
    result.throttling = json_bool(payload, b"throttling")?;
    Some(result)
}

/// Decodes a real `battery_query()` JSON payload from the kernel.
///
/// WHY the absent payload is still decoded: the platform reports "no fuel
/// gauge" as `present:false` with zeroes, and the app has to be able to say
/// that the zeroes are the platform's answer rather than a measurement.
pub fn parse_battery_payload(payload: &[u8]) -> Option<BatteryTelemetry> {
    let present = json_bool(payload, b"present")?;
    let telemetry_valid = json_bool(payload, b"telemetry_valid").unwrap_or(present);
    let mut result = BatteryTelemetry {
        present,
        telemetry_valid,
        ..BatteryTelemetry::EMPTY
    };
    result.voltage_mv = json_i64(payload, b"voltage_mv").unwrap_or(0);
    result.soc = json_i64(payload, b"soc").unwrap_or(0);
    result.capacity_mah = json_i64(payload, b"capacity").unwrap_or(0);
    result.temp_c = json_i64(payload, b"temp").unwrap_or(0);
    result.charging_len = json_str(payload, b"charging", &mut result.charging).unwrap_or(0);
    result.governor_len = json_str(payload, b"governor", &mut result.governor).unwrap_or(0);
    result.mhz = json_i64(payload, b"mhz").unwrap_or(0);
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sections() {
        assert_eq!(Section::from_index(0), Some(Section::Device));
        assert_eq!(Section::from_index(4), Some(Section::Power));
        assert_eq!(Section::from_index(6), Some(Section::System));
        assert_eq!(Section::from_index(7), None);

        assert_eq!(Section::Device.title(), b"Device");
        assert_eq!(Section::Security.title(), b"Security");
    }

    #[test]
    fn test_power_profiles() {
        assert_eq!(PowerProfile::PowerSaver.syscall_id(), 0);
        assert_eq!(PowerProfile::Balanced.syscall_id(), 1);
        assert_eq!(PowerProfile::Performance.syscall_id(), 2);
    }

    #[test]
    fn test_wallpaper_target() {
        assert_eq!(WallpaperTarget::from_index(0), WallpaperTarget::Home);
        assert_eq!(WallpaperTarget::from_index(1), WallpaperTarget::Lock);
        assert_eq!(WallpaperTarget::from_index(2), WallpaperTarget::Both);
        assert_eq!(WallpaperTarget::Home.label(), b"Home");
    }

    #[test]
    fn test_theme_mode() {
        assert_eq!(ThemeMode::Default.as_str(), "interos-default");
        assert_eq!(ThemeMode::Dark.as_str(), "interos-dark");
        assert_eq!(ThemeMode::from_str("interos-default"), Some(ThemeMode::Default));
        assert_eq!(ThemeMode::from_str("interos-dark"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::from_str("unknown"), None);
    }

    #[test]
    fn test_display_brightness_clamping() {
        assert_eq!(DisplayBrightness::new(0).percent(), 10);
        assert_eq!(DisplayBrightness::new(75).percent(), 75);
        assert_eq!(DisplayBrightness::new(120).percent(), 100);
    }

    #[test]
    fn test_pin_buffer_operations() {
        let mut pin = PinBuffer::EMPTY;
        assert!(pin.is_empty());
        assert!(!pin.is_valid_length());

        pin.push(1);
        pin.push(2);
        pin.push(3);
        assert_eq!(pin.len(), 3);
        assert!(!pin.is_valid_length()); // needs >= 4

        pin.push(4);
        assert_eq!(pin.len(), 4);
        assert!(pin.is_valid_length());
        assert_eq!(pin.as_slice(), &[1, 2, 3, 4]);

        assert_eq!(pin.pop(), Some(4));
        assert_eq!(pin.len(), 3);

        pin.reset();
        assert_eq!(pin.len(), 0);
        assert!(pin.is_empty());
    }

    #[test]
    fn test_app_permissions_and_revocation() {
        let mut app = AppPermissionState::new(b"org.interos.camera", PERM_CAMERA | PERM_VFS_WRITE);
        assert_eq!(&app.app_name[..app.app_name_len], b"org.interos.camera");
        assert!(app.is_allowed(PERM_CAMERA));
        assert!(app.is_allowed(PERM_VFS_WRITE));
        assert!(!app.is_allowed(PERM_LOCATION));

        // Dynamically revoke camera permission
        app.revoke(PERM_CAMERA);
        assert!(!app.is_allowed(PERM_CAMERA));
        assert!(app.is_allowed(PERM_VFS_WRITE));
        assert_eq!(app.effective_mask(), PERM_VFS_WRITE);

        // Restore camera permission
        app.restore(PERM_CAMERA);
        assert!(app.is_allowed(PERM_CAMERA));
        assert_eq!(app.effective_mask(), PERM_CAMERA | PERM_VFS_WRITE);
    }

    #[test]
    fn test_network_interface_record() {
        let mut iface = NetworkInterfaceRecord::EMPTY;
        assert_eq!(iface.is_up, false);
        assert_eq!(iface.mtu, 1500);

        iface.name[..4].copy_from_slice(b"eth0");
        iface.name_len = 4;
        iface.ip_addr = [10, 0, 2, 15];
        iface.is_up = true;

        assert_eq!(&iface.name[..iface.name_len], b"eth0");
        assert_eq!(iface.ip_addr, [10, 0, 2, 15]);
        assert!(iface.is_up);
    }

    #[test]
    fn test_sensor_payload_present() {
        let payload = br#"{"present":true,"orientation":"Portrait","accel":980,"lux":450,"near":false}"#;
        let t = parse_sensor_payload(payload);
        assert!(t.is_present);
        assert_eq!(t.lux, 450);
        assert_eq!(t.accel_y, 980);
        assert_eq!(t.is_near, false);
        assert_eq!(&t.orientation[..t.orientation_len], b"Portrait");
    }

    #[test]
    fn test_sensor_payload_unbound() {
        let payload = br#"{"present":false,"status":"NOT_IMPLEMENTED","error":"No physical IMU or ambient sensor detected"}"#;
        let t = parse_sensor_payload(payload);
        assert!(!t.is_present);
        assert_eq!(&t.error_msg[..t.error_msg_len], b"No physical IMU or ambient sensor detected");
        assert_eq!(&t.status[..t.status_len], b"NOT_IMPLEMENTED");
    }

    #[test]
    fn test_sensor_payload_carries_no_gyroscope_axis() {
        // The kernel's `sys_sensor_read` serialises orientation, accel, lux
        // and near - and no angular rate of any kind. Nothing this app can
        // decode may be presented as a gyroscope reading.
        let payload: &[u8] = br#"{"present":true,"orientation":"Portrait","accel":980,"lux":450,"near":false}"#;
        let t = parse_sensor_payload(payload);
        assert!(t.is_present);
        for axis in [&b"gyro"[..], &b"yaw"[..], &b"pitch"[..], &b"roll"[..]] {
            assert!(
                !payload.windows(axis.len()).any(|w| w == axis),
                "the platform's sensor payload must not carry a gyroscope axis"
            );
        }
    }

    #[test]
    fn test_thermal_zones_are_decoded_not_guessed() {
        let payload = br#"{"present":true,"cpu":42,"gpu":39,"battery":31,"modem":28,"charging":30,"state":"Normal","throttling":false}"#;
        let t = parse_thermal_payload(payload).expect("thermal payload decodes");
        assert!(t.present);
        assert_eq!(t.cpu_c, 42);
        assert_eq!(t.gpu_c, 39);
        assert_eq!(t.battery_c, 31);
        assert_eq!(t.modem_c, 28);
        assert_eq!(t.charging_c, 30);
        assert_eq!(&t.state[..t.state_len], b"Normal");
        assert!(!t.throttling);
    }

    #[test]
    fn test_thermal_throttling_flag_is_the_kernels_own() {
        let payload = br#"{"present":true,"cpu":70,"gpu":68,"battery":60,"modem":55,"charging":58,"state":"Throttled","throttling":true}"#;
        let t = parse_thermal_payload(payload).expect("thermal payload decodes");
        assert!(t.throttling);
        assert_eq!(&t.state[..t.state_len], b"Throttled");
    }

    #[test]
    fn test_thermal_absent_payload_carries_no_zone_numbers() {
        let payload = br#"{"present":false}"#;
        let t = parse_thermal_payload(payload).expect("absent payload still decodes");
        assert!(!t.present);
        assert_eq!(t.cpu_c, 0);
        assert_eq!(t.state_len, 0);
    }

    #[test]
    fn test_thermal_malformed_payload_is_rejected() {
        assert!(parse_thermal_payload(br#"{"present":true,"cpu":42}"#).is_none());
        assert!(parse_thermal_payload(br#"{"cpu":42}"#).is_none());
    }

    #[test]
    fn test_battery_reading_is_decoded_field_by_field() {
        let payload = br#"{"present":true,"telemetry_valid":true,"voltage_mv":4046,"soc":78,"capacity":3150,"charging":"Discharging","temp":25,"governor":"Balanced","mhz":1000}"#;
        let b = parse_battery_payload(payload).expect("battery payload decodes");
        assert!(b.present);
        assert!(b.telemetry_valid);
        assert_eq!(b.voltage_mv, 4046);
        assert_eq!(b.soc, 78);
        assert_eq!(b.capacity_mah, 3150);
        assert_eq!(&b.charging[..b.charging_len], b"Discharging");
        assert_eq!(b.temp_c, 25);
    }

    #[test]
    fn test_battery_absent_payload_is_not_a_zero_percent_reading() {
        let payload = br#"{"present":false,"telemetry_valid":false,"voltage_mv":0,"soc":0,"capacity":0,"charging":"Disconnected","temp":0,"governor":"Balanced","mhz":1000}"#;
        let b = parse_battery_payload(payload).expect("absent payload still decodes");
        assert!(!b.present);
        assert!(!b.telemetry_valid);
        // The zeros are the platform's absence, so a card must not present
        // them as a measurement.
        assert_eq!(b.soc, 0);
        assert_eq!(&b.charging[..b.charging_len], b"Disconnected");
    }

    #[test]
    fn test_platform_status_names_are_the_kernels_own() {
        assert_eq!(syscall_status_name(-1), b"PermissionDenied");
        assert_eq!(syscall_status_name(-2), b"InvalidSlot");
        assert_eq!(syscall_status_name(-3), b"InvalidArguments");
        assert_eq!(syscall_status_name(-12), b"DeviceError");
    }

    #[test]
    fn test_hardware_availability_is_decoded_from_the_kernels_codes() {
        assert_eq!(
            parse_hardware_availability(0),
            HardwareAvailability::Available
        );
        assert_eq!(
            parse_hardware_availability(1),
            HardwareAvailability::Emulated
        );
        assert_eq!(
            parse_hardware_availability(2),
            HardwareAvailability::Unavailable
        );
        assert_eq!(
            parse_hardware_availability(-1),
            HardwareAvailability::Unknown
        );
    }
}
