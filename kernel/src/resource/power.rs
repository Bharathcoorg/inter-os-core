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
            display_refresh_hz: 120, // 120Hz ProMotion display
            battery_percent: 100,
            is_charging: true,
            touch_boost_active: false,
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
        self.battery_percent = percent;
        self.is_charging = is_charging;
        self.recalculate_frequency();
    }

    /// Recalculates effective clock frequency and display refresh rate based on policy.
    fn recalculate_frequency(&mut self) {
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
