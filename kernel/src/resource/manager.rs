//! Unified Dynamic Resource Manager
//!
//! Enforces system-wide hardware resource budgets across CPU, RAM, Storage,
//! Bandwidth, Battery, and Thermal conditions.
//! Resource Principle #7: Dynamic resource budgeting to prevent RAM/storage exhaustion.
//! Resource Principle #8: Foreground user workloads have strict priority over
//! background tasks and state synchronization.

/// Workload priority tiers defined in master architecture specification.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WorkloadTier {
    KernelSafety = 1,
    ForegroundUserApp = 2,
    ExplicitUserOperation = 3,
    CriticalCommunications = 4,
    OsServices = 5,
    BackgroundSync = 6,
    BackgroundNetwork = 7,
    BackgroundTasks = 8,
    Maintenance = 9,
}

/// Battery power state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BatteryStatus {
    /// Charge percentage (0 - 100). Only meaningful when `present` is true.
    pub percent: u8,
    /// Whether external AC / USB charging power is connected.
    pub is_charging: bool,
    /// Whether a real fuel gauge has reported at least once. When false the
    /// remaining fields carry no measurement and must not be presented as one.
    pub present: bool,
}

/// Thermal condition state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ThermalStatus {
    /// Temperature in degrees Celsius. Only meaningful when `present` is true.
    pub temp_celsius: u8,
    /// Whether thermal throttling has been engaged.
    pub throttling: bool,
    /// Whether a real thermal sensor has reported at least once.
    pub present: bool,
}

/// Unified Resource Manager configuration and telemetry.
pub struct ResourceManager {
    pub battery: BatteryStatus,
    pub thermal: ThermalStatus,
    pub ram_total_bytes: usize,
    pub ram_allocated_bytes: usize,
    pub ram_foreground_bytes: usize,
    pub active_foreground_task: bool,
}

impl ResourceManager {
    /// Creates a ResourceManager reporting no telemetry.
    ///
    /// No fuel gauge or thermal sensor has been bound at this point, so both
    /// readings start absent at zero rather than at a plausible-looking value.
    /// Policy that consumes these fields must check `present` first.
    pub const fn new() -> Self {
        Self {
            battery: BatteryStatus {
                percent: 0,
                is_charging: false,
                present: false,
            },
            thermal: ThermalStatus {
                temp_celsius: 0,
                throttling: false,
                present: false,
            },
            ram_total_bytes: 1024 * 1024, // 1 MiB kernel heap
            ram_allocated_bytes: 0,
            ram_foreground_bytes: 0,
            active_foreground_task: false,
        }
    }

    /// Records a battery reading from a real fuel gauge.
    pub fn set_battery_telemetry(&mut self, percent: u8, is_charging: bool) {
        self.battery = BatteryStatus {
            percent: percent.min(100),
            is_charging,
            present: true,
        };
    }

    /// Records a thermal reading from a real sensor.
    pub fn set_thermal_telemetry(&mut self, temp_celsius: u8, throttling: bool) {
        self.thermal = ThermalStatus {
            temp_celsius,
            throttling,
            present: true,
        };
    }

    /// Evaluates whether a workload tier is permitted to execute given current power, thermal, and task priorities.
    pub fn is_tier_permitted(&self, tier: WorkloadTier) -> bool {
        // Critical safety and foreground always permitted
        if tier <= WorkloadTier::CriticalCommunications {
            return true;
        }

        // Thermal throttling pauses background network and lower tiers
        if self.thermal.throttling && tier >= WorkloadTier::BackgroundNetwork {
            return false;
        }

        // Low battery (< 20% and not charging) pauses background network and lower tiers
        if !self.battery.is_charging && self.battery.percent < 20 && tier >= WorkloadTier::BackgroundNetwork {
            return false;
        }

        // When a foreground user task is intensely active, background tasks and maintenance are paused
        if self.active_foreground_task && tier >= WorkloadTier::BackgroundTasks {
            return false;
        }

        true
    }

    /// Sets foreground activity state, triggering preemption of lower tiers if active.
    pub fn set_foreground_active(&mut self, active: bool) {
        self.active_foreground_task = active;
    }

    /// Updates live battery status.
    pub fn update_battery(&mut self, percent: u8, is_charging: bool) {
        self.battery.percent = percent.min(100);
        self.battery.is_charging = is_charging;
    }

    /// Updates live thermal status.
    pub fn update_thermal(&mut self, temp_celsius: u8) {
        self.thermal.temp_celsius = temp_celsius;
        self.thermal.throttling = temp_celsius >= 75;
    }
}
