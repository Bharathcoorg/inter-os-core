//! Persistent system settings service & hardware capability inventory.
//!
//! Settings are authoritative kernel state mirrored to a crash-consistent ATA
//! journal. Userspace accesses them through capability-checked syscalls.
//! Includes explicit hardware availability auditing so features without physical
//! or virtual hardware report 'unavailable' rather than pretending to function.

use alloc::vec::Vec;
use spin::Mutex;

const MAX: usize = 4096;
const KIND: u8 = 3;

/// Hardware availability status for system settings and features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareAvailability {
    /// Hardware controller is physically present and operating.
    Available,
    /// Controller is simulated through a virtual device (e.g. QEMU).
    SimulatedVirtual,
    /// Hardware controller is not present on this platform.
    Unavailable,
}

impl HardwareAvailability {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::SimulatedVirtual => "simulated",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Standard setting category namespaces.
pub const SETTING_CATEGORIES: &[&'static str] = &[
    "wifi",
    "bluetooth",
    "cellular",
    "display",
    "sound",
    "notifications",
    "battery",
    "storage",
    "apps",
    "permissions",
    "security",
    "privacy",
    "users",
    "accessibility",
    "developer",
    "updates",
    "datetime",
    "language",
    "about",
];

/// Returns the honest hardware availability status for a setting category.
pub fn get_hardware_status(category: &str) -> HardwareAvailability {
    match category {
        "display" => HardwareAvailability::Available,     // DRM/KMS scanout 720x1440
        "storage" => HardwareAvailability::Available,     // Primary ATA + Persistent secondary ATA
        "sound" => HardwareAvailability::Available,       // PC speaker hardware timer
        "battery" => HardwareAvailability::SimulatedVirtual, // QEMU PMIC battery model
        "sensors" => HardwareAvailability::SimulatedVirtual, // QEMU virtual sensor bus
        "wifi" => HardwareAvailability::Unavailable,      // No physical Wi-Fi controller on x86 virt
        "bluetooth" => HardwareAvailability::Unavailable, // No physical BT controller on x86 virt
        "cellular" => HardwareAvailability::Unavailable,  // No physical baseband modem on x86 virt
        _ => HardwareAvailability::Available,
    }
}

pub struct SettingsStore {
    data: Vec<u8>,
    generation: u64,
}

impl SettingsStore {
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            generation: 0,
        }
    }

    fn find(&self, key: &str) -> Option<(usize, usize)> {
        let mut start = 0;
        while start < self.data.len() {
            let end = self.data[start..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|x| start + x)
                .unwrap_or(self.data.len());
            let line = &self.data[start..end];
            if let Some(eq) = line.iter().position(|b| *b == b'=') {
                if core::str::from_utf8(&line[..eq]).ok() == Some(key) {
                    return Some((start, end));
                }
            }
            start = end.saturating_add(1);
        }
        None
    }

    pub fn get(&self, key: &str, out: &mut [u8]) -> usize {
        let Some((start, end)) = self.find(key) else {
            return 0;
        };
        let line = &self.data[start..end];
        let eq = line.iter().position(|b| *b == b'=').unwrap_or(line.len());
        let value = &line[eq.saturating_add(1)..];
        let n = value.len().min(out.len());
        out[..n].copy_from_slice(&value[..n]);
        n
    }

    pub fn set(&mut self, key: &str, value: &[u8]) -> Result<(), ()> {
        if key.is_empty()
            || key.len() > 96
            || value.len() > 512
            || key.as_bytes().iter().any(|b| *b == b'\n' || *b == b'=')
            || value.iter().any(|b| *b == b'\n')
        {
            return Err(());
        }
        let mut next = Vec::with_capacity(MAX);
        let replacement_len = key.len() + 1 + value.len() + 1;
        let old = self.find(key);
        let mut pos = 0;
        let mut replaced = false;
        while pos < self.data.len() {
            let end = self.data[pos..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|x| pos + x)
                .unwrap_or(self.data.len());
            let is_old = old.map(|(s, e)| s == pos && e == end).unwrap_or(false);
            if !is_old {
                if next.len() + end - pos + 1 > MAX {
                    return Err(());
                }
                next.extend_from_slice(&self.data[pos..end]);
                next.push(b'\n');
            } else {
                if next.len() + replacement_len > MAX {
                    return Err(());
                }
                next.extend_from_slice(key.as_bytes());
                next.push(b'=');
                next.extend_from_slice(value);
                next.push(b'\n');
                replaced = true;
            }
            pos = end.saturating_add(1);
        }
        if !replaced {
            if next.len() + replacement_len > MAX {
                return Err(());
            }
            next.extend_from_slice(key.as_bytes());
            next.push(b'=');
            next.extend_from_slice(value);
            next.push(b'\n');
        }
        let generation = crate::storage::persistent::commit_kind(&next, KIND).map_err(|_| ())?;
        self.data = next;
        self.generation = generation;
        Ok(())
    }

    pub fn snapshot(&self, out: &mut [u8]) -> usize {
        let n = self.data.len().min(out.len());
        out[..n].copy_from_slice(&self.data[..n]);
        n
    }
}

pub static SETTINGS: Mutex<SettingsStore> = Mutex::new(SettingsStore::new());

pub fn init() {
    let mut buf = [0u8; MAX];
    let mut s = SETTINGS.lock();
    if let Ok(Some((generation, len))) = crate::storage::persistent::load_latest_kind(&mut buf, KIND) {
        s.data.extend_from_slice(&buf[..len]);
        s.generation = generation;
    } else {
        let defaults = concat!(
            "power.profile=balanced\n",
            "ui.theme=interos-default\n",
            "ui.brightness.auto=true\n",
            "wifi.hardware=unavailable\n",
            "bluetooth.hardware=unavailable\n",
            "cellular.hardware=unavailable\n",
            "display.resolution=720x1440\n",
            "sound.volume=75\n",
            "notifications.dnd=false\n",
            "security.lock_timeout=30\n",
            "users.multiuser_enabled=true\n",
            "about.os_name=InterOS Core\n",
            "about.os_version=0.8.2\n"
        ).as_bytes();

        if let Ok(g) = crate::storage::persistent::commit_kind(defaults, KIND) {
            s.data.extend_from_slice(defaults);
            s.generation = g;
        }
    }
}
