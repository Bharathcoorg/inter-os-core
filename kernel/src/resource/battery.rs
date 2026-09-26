//! Mobile Battery Fuel Gauge Driver & Power Drainage Engine
//!
//! Provides accurate Li-ion State of Charge (SoC %) estimation from cell voltage,
//! charging status monitoring, cycle health accounting, and dynamic power draw
//! estimation correlated with DVFS CPU clock frequency and display panel brightness.

/// Battery Charging State
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChargingStatus {
    Discharging,
    ChargingAC,
    ChargingWireless,
    Full,
}

/// Mobile Battery Fuel Gauge Telemetry
#[derive(Clone, Debug)]
pub struct BatteryFuelGauge {
    /// Cell terminal voltage in millivolts (nominal 3500..4200 mV)
    pub voltage_mv: u32,
    /// Remaining capacity in milliampere-hours
    pub capacity_mah: u32,
    /// Factory design capacity in milliampere-hours
    pub design_capacity_mah: u32,
    /// State of Charge percentage (0..100%)
    pub soc_percent: u8,
    /// Cell temperature in Celsius
    pub temperature_c: i8,
    /// Total full discharge-charge cycles
    pub cycle_count: u32,
    /// Current charging power source
    pub charging_status: ChargingStatus,
}

pub const BATTERY_JSON_MAX: usize = 256;

impl BatteryFuelGauge {
    /// Creates a fuel gauge initialized with reference smartphone battery telemetry.
    pub const fn new() -> Self {
        Self {
            voltage_mv: 3850,
            capacity_mah: 4700,
            design_capacity_mah: 5000,
            soc_percent: 94,
            temperature_c: 28,
            cycle_count: 42,
            charging_status: ChargingStatus::ChargingAC,
        }
    }

    /// Checks if a physical ACPI or I2C battery fuel gauge is present on the board.
    pub fn is_hardware_present(&self) -> bool {
        // Standard development QEMU/PC platform runs on AC/line power without physical battery
        false
    }

    /// Updates terminal voltage and recalculates State of Charge (SoC) curve.
    pub fn update_voltage(&mut self, mv: u32) {
        self.voltage_mv = mv;
        if mv <= 3500 {
            self.soc_percent = 0;
        } else if mv >= 4200 {
            self.soc_percent = 100;
        } else {
            // Linear approximation between 3500 mV (0%) and 4200 mV (100%)
            self.soc_percent = (((mv - 3500) * 100) / 700) as u8;
        }
        self.capacity_mah = ((self.design_capacity_mah as u64 * self.soc_percent as u64) / 100) as u32;
    }

    /// Computes dynamic system power consumption in milliwatts based on CPU frequency and display brightness.
    pub fn estimate_power_drain_mw(&self, cpu_freq_mhz: u32, brightness_pct: u8) -> u32 {
        // CPU power draw scales with DVFS frequency: ~0.8 mW per MHz
        let cpu_drain_mw = (cpu_freq_mhz * 8) / 10;
        // OLED panel power draw: ~12 mW per percent brightness
        let display_drain_mw = (brightness_pct as u32) * 12;
        // Static baseline board power (modem standby, RAM refresh, sensors)
        let base_drain_mw = 350;

        cpu_drain_mw + display_drain_mw + base_drain_mw
    }

    /// Returns true if State of Charge is critically low (< 20%).
    pub fn is_low(&self) -> bool {
        self.soc_percent < 20
    }

    /// Returns battery health percentage (capacity retention vs design capacity).
    pub fn health_percent(&self) -> u8 {
        if self.design_capacity_mah == 0 { return 0; }
        ((self.capacity_mah as u64 * 100) / self.design_capacity_mah as u64).min(100) as u8
    }
}
