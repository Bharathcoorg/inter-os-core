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
}

