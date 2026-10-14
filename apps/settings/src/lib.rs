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
}
