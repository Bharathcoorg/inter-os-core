//! Mobile DVFS Power & Thermal Governor
//!
//! Dynamic Voltage and Frequency Scaling (DVFS) and display refresh rate
//! governor tailored for AI-native smartphones.
//! Coordinates with the 9-Tier Resource Manager to balance responsiveness,
//! thermal headroom, and battery runtime.

/// Dynamic Voltage and Frequency Scaling (DVFS) governor policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DvfsGovernor {
    /// Locks CPU frequency to maximum clock rate; maximum responsiveness.
    Performance = 0,
    /// Locks CPU frequency to minimum clock rate; maximum battery savings.
    Powersave = 1,
    /// Dynamically scales clock frequency based on touch interactions and foreground tasks.
    Interactive = 2,
    /// Energy-aware scheduler-guided clock frequency scaling.
    Schedutil = 3,
}

/// Thermal trip points for mobile SoC safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalTrip {
    /// Nominal (< 60°C): 100% clock rate, 120Hz display refresh.
    Nominal,
    /// Warm (60°C - 74°C): 75% clock rate, 90Hz display refresh.
    Warning,
    /// Hot (75°C - 84°C): 50% clock rate, 60Hz display refresh, background AI paused.
    Throttling,
    /// Critical (>= 85°C): 25% emergency clock rate, 30Hz display refresh, emergency cooldown.
    Critical,
}

/// Dynamic power management state of the mobile device.
#[derive(Debug, Clone)]
pub struct PowerManager {
    pub governor: DvfsGovernor,
    pub base_freq_mhz: u32,
    pub current_freq_mhz: u32,
    pub max_freq_mhz: u32,
    pub min_freq_mhz: u32,
    pub thermal_trip: ThermalTrip,
    pub display_refresh_hz: u16,
    pub battery_percent: u8,
    pub is_charging: bool,
    pub touch_boost_active: bool,
    pub telemetry_valid: bool,
    pub thermal_valid: bool,
    pub battery_valid: bool,
}

impl PowerManager {
    pub const fn new(base_freq_mhz: u32) -> Self {
        Self {
            governor: DvfsGovernor::Interactive,
            base_freq_mhz,
            current_freq_mhz: base_freq_mhz,
            max_freq_mhz: base_freq_mhz,
            min_freq_mhz: base_freq_mhz / 4, // 25% minimum frequency floor
            thermal_trip: ThermalTrip::Nominal,
            display_refresh_hz: 0,
            battery_percent: 0,
            is_charging: false,
            touch_boost_active: false,
            telemetry_valid: false,
            thermal_valid: false,
            battery_valid: false,
        }
    }

    /// Changes the active DVFS governor.
    pub fn set_governor(&mut self, governor: DvfsGovernor) {
        self.governor = governor;
        self.recalculate_frequency();
    }

    /// Triggers immediate CPU frequency boost on user touch screen interaction.
    pub fn on_touch_interaction(&mut self) {
        if self.governor == DvfsGovernor::Interactive || self.governor == DvfsGovernor::Schedutil {
            self.touch_boost_active = true;
            self.recalculate_frequency();
        }
    }

    /// Clears touch boost once interaction settles.
    pub fn clear_touch_boost(&mut self) {
        self.touch_boost_active = false;
        self.recalculate_frequency();
    }

    /// Updates thermal status and adjusts thermal trip points.
    pub fn update_thermal(&mut self, temperature_c: u8) {
        self.thermal_valid = true;
        self.telemetry_valid = self.thermal_valid || self.battery_valid;
        self.thermal_trip = if temperature_c >= 85 {
            ThermalTrip::Critical
        } else if temperature_c >= 75 {
            ThermalTrip::Throttling
        } else if temperature_c >= 60 {
            ThermalTrip::Warning
        } else {
            ThermalTrip::Nominal
        };
        self.recalculate_frequency();
    }

    /// Updates battery charge level and charging status.
    pub fn update_battery(&mut self, percent: u8, is_charging: bool) {
        self.battery_valid = true;
        self.telemetry_valid = self.thermal_valid || self.battery_valid;
        self.battery_percent = percent;
        self.is_charging = is_charging;
        self.recalculate_frequency();
    }

    /// Recalculates effective clock frequency and display refresh rate based on policy.
    fn recalculate_frequency(&mut self) {
        if !self.telemetry_valid {
            self.display_refresh_hz = 0;
            return;
        }
        match self.governor {
            DvfsGovernor::Performance => {
                // Performance locks to max unless critical thermal
                if self.thermal_trip == ThermalTrip::Critical {
                    self.current_freq_mhz = self.base_freq_mhz / 2;
                    self.display_refresh_hz = 60;
                } else {
                    self.current_freq_mhz = self.max_freq_mhz;
                    self.display_refresh_hz = 120;
                }
            }

            DvfsGovernor::Powersave => {
                self.current_freq_mhz = self.min_freq_mhz;
                self.display_refresh_hz = 60;
            }

            DvfsGovernor::Interactive | DvfsGovernor::Schedutil => {
                // Base frequency scaling on thermal and battery state
                match self.thermal_trip {
                    ThermalTrip::Critical => {
                        self.current_freq_mhz = self.min_freq_mhz;
                        self.display_refresh_hz = 30;
                    }
                    ThermalTrip::Throttling => {
                        self.current_freq_mhz = self.base_freq_mhz / 2;
                        self.display_refresh_hz = 60;
                    }
                    ThermalTrip::Warning => {
                        let target = (self.base_freq_mhz as u64 * 3 / 4) as u32;
                        self.current_freq_mhz = target;
                        self.display_refresh_hz = 90;
                    }
                    ThermalTrip::Nominal => {
                        if !self.is_charging && self.battery_percent < 15 {
                            // Low-power battery saver
                            self.current_freq_mhz = self.base_freq_mhz / 2;
                            self.display_refresh_hz = 60;
                        } else if self.touch_boost_active {
                            // Instant ramp to 100% on touch
                            self.current_freq_mhz = self.max_freq_mhz;
                            self.display_refresh_hz = 120;
                        } else {
                            // Balanced idle
                            self.current_freq_mhz = (self.base_freq_mhz as u64 * 4 / 5) as u32;
                            self.display_refresh_hz = 120;
                        }
                    }
                }
            }
        }
    }
}


pub trait PowerDomain {
    fn prepare_suspend(&mut self) -> Result<(), &'static str>;
    fn suspend(&mut self) -> Result<(), &'static str>;
    fn resume(&mut self) -> Result<(), &'static str>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerDomainMask(pub u32);

impl PowerDomainMask {
    pub const CPU: u32 = 1 << 0;
    pub const GPU: u32 = 1 << 1;
    pub const DISPLAY: u32 = 1 << 2;
    pub const MODEM: u32 = 1 << 3;
    pub const WIFI: u32 = 1 << 4;
    pub const BLUETOOTH: u32 = 1 << 5;
    pub const SENSORS: u32 = 1 << 6;
    pub const AUDIO: u32 = 1 << 7;
    pub const fn all() -> Self { Self(Self::CPU|Self::GPU|Self::DISPLAY|Self::MODEM|Self::WIFI|Self::BLUETOOTH|Self::SENSORS|Self::AUDIO) }
    pub const fn contains(self, bit:u32)->bool { self.0 & bit != 0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SuspendState { Active=0, Preparing=1, Suspended=2, Resuming=3 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WakeSource { Timer=0, PowerButton=1, Touch=2, Network=3, Modem=4, Alarm=5 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerTransition {
    pub state: SuspendState,
    pub wake_mask: u32,
    pub entered_at: u64,
    pub suspended_domains: PowerDomainMask,
}

impl PowerTransition {
    pub const fn new() -> Self { Self { state:SuspendState::Active, wake_mask:0, entered_at:0, suspended_domains:PowerDomainMask(0) } }
    pub fn enable_wake(&mut self, source:WakeSource) { self.wake_mask |= 1u32 << source as u8; }
    pub fn disable_wake(&mut self, source:WakeSource) { self.wake_mask &= !(1u32 << source as u8); }
    pub fn wake_enabled(&self, source:WakeSource)->bool { self.wake_mask & (1u32 << source as u8) != 0 }
    pub fn prepare_suspend(&mut self, now:u64)->Result<(),&'static str>{
        if self.state != SuspendState::Active { return Err("power transition already active"); }
        if self.wake_mask == 0 { return Err("no wake source configured"); }
        self.state=SuspendState::Preparing; self.entered_at=now; Ok(())
    }
    pub fn enter_suspend(&mut self)->Result<(),&'static str>{
        if self.state != SuspendState::Preparing { return Err("suspend not prepared"); }
        self.state=SuspendState::Suspended; self.suspended_domains=PowerDomainMask::all(); Ok(())
    }
    pub fn wake(&mut self, source:WakeSource)->Result<(),&'static str>{
        if self.state != SuspendState::Suspended || !self.wake_enabled(source) { return Err("wake source not armed"); }
        self.state=SuspendState::Resuming; Ok(())
    }
    pub fn resume_complete(&mut self)->Result<(),&'static str>{
        if self.state != SuspendState::Resuming { return Err("resume not pending"); }
        self.state=SuspendState::Active; self.suspended_domains=PowerDomainMask(0); Ok(())
    }
}
