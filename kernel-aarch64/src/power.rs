// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! AArch64 Battery, Thermal & DVFS Power Governor (mobile profile)
//!
//! This module is a **thermal/DVFS policy engine and capability boundary, not a PMIC driver**.
//! No AXP803/PM8998/MT6358 fuel gauge or TSENS array is described, addressed, or emulated here.
//! Policy decisions (which thermal tier applies, which clock the policy asks for, whether to cap
//! refresh rate) are computed software and are kept; every *reading* those decisions depend on must
//! come from a bound PMIC/thermal driver.
//!
//! Status of each layer, stated explicitly:
//! - **Unavailable**: battery voltage, capacity, charge current, die temperature, and the actual
//!   CPU clock. With no fuel gauge bound, `current_ma` and `temperature_deci_c` are `None` and the
//!   governor reports no applied frequency rather than asserting one it cannot set.
//! - **Emulated**: none. The constructor does not invent discharge current or die temperature.
//! - **Present**: none. Zero PMIC/thermal hardware is bound at build time.
//!
//! Reusable software:
//! - 4-tier thermal mitigation policy
//! - Battery-saver evaluation
//! - DVFS target selection, clamped to the operating points a bound clock controller reports
//! - Battery-driven display refresh capping
//! - Architectural `wfi` (Wait For Interrupt) low-power sleep

/// No CPU frequency is applied because no clock controller is bound.
pub const NO_FREQ_MHZ: u32 = 0;
/// No display refresh target is applied because no panel is bound.
pub const NO_REFRESH_TARGET_HZ: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargingState {
    Charging,
    Discharging,
    Full,
    NotCharging,
}

#[derive(Debug, Clone, Copy)]
pub struct BatteryStatus {
    /// True only when a concrete PMIC driver has bound and populated this reading.
    pub hardware_present: bool,
    pub voltage_mv: u32,
    pub capacity_pct: u8,
    /// Charge/discharge current in milliamperes; `None` when never measured.
    pub current_ma: Option<i32>,
    /// Die temperature in tenths of a degree Celsius; `None` when never measured.
    pub temperature_deci_c: Option<i32>,
    pub state: ChargingState,
}

impl BatteryStatus {
    pub const fn new(voltage_mv: u32, capacity_pct: u8, state: ChargingState) -> Self {
        Self {
            hardware_present: false,
            voltage_mv,
            capacity_pct,
            // WHY None: the previous seeds `current_ma: -450` ("450mA typical active phone
            // discharge") and `temperature_deci_c: 315` ("31.5 C") were invented. Charge current
            // and die temperature are measurements; a constructor may not produce one.
            current_ma: None,
            temperature_deci_c: None,
            state,
        }
    }

    /// True only when a bound fuel gauge supplied both current and temperature.
    pub const fn has_full_telemetry(&self) -> bool {
        self.hardware_present && self.current_ma.is_some() && self.temperature_deci_c.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalPolicy {
    Normal,          // < 65 C: highest clock in the policy table
    Warm,            // 65-74 C: reduced clock, no refresh cap
    ThrottlingTier1, // 75-84 C: further reduced clock, cap display refresh
    CriticalCooldown,// >= 85 C: lowest clock in the policy table, lowest refresh target
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorMode {
    Performance,
    Powersave,
    Interactive,
    Schedutil,
}

pub struct DvfsGovernor {
    pub hardware_present: bool,
    /// Clock the policy last asked for, in MHz. `NO_FREQ_MHZ` means no clock has been applied,
    /// because `min_freq_mhz`/`max_freq_mhz` are only known once a controller reports them.
    pub current_freq_mhz: u32,
    /// Lowest operating point, supplied by a bound clock controller; `0` until then.
    pub min_freq_mhz: u32,
    /// Highest operating point, supplied by a bound clock controller; `0` until then.
    pub max_freq_mhz: u32,
    pub mode: GovernorMode,
    pub current_thermal_policy: ThermalPolicy,
    pub battery_saver_active: bool,
    /// Refresh target the policy asks for, in Hz. `0` means no target has been applied, because no
    /// panel's supported range is known without a bound display controller.
    pub display_target_fps: u32,
}

impl DvfsGovernor {
    pub const fn new() -> Self {
        Self {
            hardware_present: false,
            // WHY 0, not 2400: asserting a 2400 MHz clock with no clock controller claims a
            // frequency this kernel cannot produce.
            current_freq_mhz: NO_FREQ_MHZ,
            // WHY 0, not 600/2400: operating points are properties of a specific clock tree.
            // Until a driver reports them, both bounds are unknown and stay 0.
            min_freq_mhz: NO_FREQ_MHZ,
            max_freq_mhz: NO_FREQ_MHZ,
            mode: GovernorMode::Interactive,
            current_thermal_policy: ThermalPolicy::Normal,
            battery_saver_active: false,
            // WHY 0, not 120: no panel is bound, so no supported refresh range exists and no
            // 120 Hz panel is implied.
            display_target_fps: NO_REFRESH_TARGET_HZ,
        }
    }

    pub fn set_hardware_present(&mut self, present: bool) { self.hardware_present = present; }

    /// Records the clock operating points reported by a bound clock controller.
    ///
    /// Without this the governor has no frequency range to clamp against and stays at
    /// `NO_FREQ_MHZ`; the bounds must not be guessed.
    pub fn set_operating_points(&mut self, min_mhz: u32, max_mhz: u32) {
        if min_mhz > max_mhz {
            return;
        }
        self.min_freq_mhz = min_mhz;
        self.max_freq_mhz = max_mhz;
    }

    pub fn update(&mut self, cpu_temp_c: i32, battery: &BatteryStatus) -> ThermalPolicy {
        if !self.hardware_present || !battery.hardware_present {
            self.current_thermal_policy = ThermalPolicy::Normal;
            self.battery_saver_active = false;
            // WHY: no clock controller means no frequency range to clamp to and no panel means no
            // refresh rate to request. 0 means "nothing applied", which is the honest value; the
            // previous code implied a live 600 MHz clock and a 30 Hz panel were in effect.
            self.current_freq_mhz = NO_FREQ_MHZ;
            self.display_target_fps = NO_REFRESH_TARGET_HZ;
            return self.current_thermal_policy;
        }
        // Evaluate thermal policy
        self.current_thermal_policy = if cpu_temp_c >= 85 {
            ThermalPolicy::CriticalCooldown
        } else if cpu_temp_c >= 75 {
            ThermalPolicy::ThrottlingTier1
        } else if cpu_temp_c >= 65 {
            ThermalPolicy::Warm
        } else {
            ThermalPolicy::Normal
        };

        // Evaluate battery saver
        self.battery_saver_active = battery.capacity_pct <= 15 && battery.state == ChargingState::Discharging;

        // Apply DVFS frequency and display target.
        // These numbers are POLICY TARGETS, not measurements of this device: what the policy would
        // request, clamped to the operating points a bound clock controller reported. With no
        // controller bound, min/max are 0 and every target clamps to NO_FREQ_MHZ.
        let requested_mhz = match self.current_thermal_policy {
            ThermalPolicy::CriticalCooldown => 600,
            ThermalPolicy::ThrottlingTier1 => 1200,
            ThermalPolicy::Warm => if self.battery_saver_active { 1200 } else { 1800 },
            ThermalPolicy::Normal => if self.battery_saver_active { 1200 } else { 2400 },
        };
        self.current_freq_mhz = requested_mhz.clamp(self.min_freq_mhz, self.max_freq_mhz);

        // Refresh targets are likewise requests pending a panel's advertised support. A frequency
        // range of 0 means no clock controller reported in, so no refresh request is issued.
        let requested_hz = match self.current_thermal_policy {
            ThermalPolicy::CriticalCooldown => 30,
            ThermalPolicy::ThrottlingTier1 => 60,
            ThermalPolicy::Warm | ThermalPolicy::Normal => if self.battery_saver_active { 60 } else { 120 },
        };
        self.display_target_fps =
            if self.max_freq_mhz == NO_FREQ_MHZ { NO_REFRESH_TARGET_HZ } else { requested_hz };

        self.current_thermal_policy
    }

    pub fn enter_low_power_idle(&self) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}
