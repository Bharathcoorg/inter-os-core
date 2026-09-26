//! Dynamic Multi-Zone Thermal Governor & Hardware Throttle Engine
//!
//! Continuously tracks thermal sensors across mobile SoC domains (CPU, GPU, Battery,
//! 5G Modem, Charging IC). Implements multi-tier trip points and progressive cooling
//! strategies (task throttling, background compute pause, DVFS frequency clamping)
//! to prevent mobile thermal runaway during intensive AI workloads.

use spin::Mutex;

pub const THERMAL_MIN_C: i32 = -40;
pub const THERMAL_MAX_C: i32 = 150;
pub const THERMAL_JSON_MAX: usize = 256;

/// Monitored hardware thermal zones across the mobile SoC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ThermalZone {
    CpuJunction = 0,
    GpuCore = 1,
    BatteryCell = 2,
    Modem5G = 3,
    ChargingIc = 4,
}

/// Progressive mobile cooling and throttle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolingState {
    Normal,            // < 45°C: Unrestricted execution
    Warm,              // 45°C - 55°C: Gradient monitoring, nominal DVFS
    Alert,             // 55°C - 65°C: Background tasks and network down-clocked
    Throttled,         // 65°C - 80°C: DVFS clamped to Powersave, mandatory task yield pauses
    CriticalShutdown,  // > 80°C: Emergency task freeze to prevent silicon damage
}

/// Dynamic Thermal Governor.
pub struct ThermalManager {
    zone_temperatures: [i32; 5], // Temperatures in °C
    active_cooling_state: CoolingState,
    throttle_delay_micros: u32,
}

impl ThermalManager {
    pub const fn new() -> Self {
        Self {
            zone_temperatures: [42, 39, 34, 38, 36], // Default nominal idle temperatures
            active_cooling_state: CoolingState::Normal,
            throttle_delay_micros: 0,
        }
    }

    /// Updates temperature reading for a specific zone and re-evaluates governor policy.
    pub fn update_temperature(&mut self, zone: ThermalZone, temp_c: i32) {
        let idx = zone as usize;
        if idx < self.zone_temperatures.len() {
            self.zone_temperatures[idx] = temp_c.clamp(THERMAL_MIN_C, THERMAL_MAX_C);
        }
        self.evaluate_policy();
    }

    /// Returns the highest temperature across all monitored hardware zones.
    pub fn max_temperature(&self) -> i32 {
        let mut max_t = self.zone_temperatures[0];
        let mut i = 1;
        while i < self.zone_temperatures.len() {
            if self.zone_temperatures[i] > max_t {
                max_t = self.zone_temperatures[i];
            }
            i += 1;
        }
        max_t
    }

    /// Recomputes cooling state and throttle delays based on trip points.
    pub fn evaluate_policy(&mut self) -> CoolingState {
        let max_t = self.max_temperature();
        let state = if max_t >= 80 {
            self.throttle_delay_micros = 50_000; // 50 ms delay
            CoolingState::CriticalShutdown
        } else if max_t >= 65 {
            self.throttle_delay_micros = 20_000; // 20 ms delay
            CoolingState::Throttled
        } else if max_t >= 55 {
            self.throttle_delay_micros = 5_000;  // 5 ms delay
            CoolingState::Alert
        } else if max_t >= 45 {
            self.throttle_delay_micros = 0;
            CoolingState::Warm
        } else {
            self.throttle_delay_micros = 0;
            CoolingState::Normal
        };
        self.active_cooling_state = state;
        state
    }

    /// Returns the active cooling state.
    pub fn cooling_state(&self) -> CoolingState {
        self.active_cooling_state
    }

    /// Returns whether background workloads must be throttled.
    pub fn is_throttling(&self) -> bool {
        matches!(
            self.active_cooling_state,
            CoolingState::Throttled | CoolingState::CriticalShutdown
        )
    }

    /// Returns the enforced throttle delay for background tasks in microseconds.
    pub fn throttle_delay_micros(&self) -> u32 {
        self.throttle_delay_micros
    }

    /// Serializes thermal zone telemetry into a standardized JSON representation.
    pub fn format_json(&self, out: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct SliceWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl<'a> Write for SliceWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let rem = self.buf.len().saturating_sub(self.pos);
                let to_write = bytes.len().min(rem);
                self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                self.pos += to_write;
                Ok(())
            }
        }
        if out.len() < THERMAL_JSON_MAX { return 0; }
        let mut writer = SliceWriter { buf: out, pos: 0 };
        let state_str = match self.active_cooling_state {
            CoolingState::Normal => "Normal",
            CoolingState::Warm => "Warm",
            CoolingState::Alert => "Alert",
            CoolingState::Throttled => "Throttled",
            CoolingState::CriticalShutdown => "Critical",
        };
        let _ = write!(
            writer,
            r#"{{"cpu":{},"gpu":{},"battery":{},"modem":{},"charging":{},"state":"{}","throttling":{}}}"#,
            self.zone_temperatures[0],
            self.zone_temperatures[1],
            self.zone_temperatures[2],
            self.zone_temperatures[3],
            self.zone_temperatures[4],
            state_str,
            self.is_throttling()
        );
        writer.pos
    }
}

pub static THERMAL_MANAGER: Mutex<ThermalManager> = Mutex::new(ThermalManager::new());

/// Initializes the Multi-Zone Thermal Governor and executes automated trip point tests.
pub fn init() {
    let mut tm = THERMAL_MANAGER.lock();

    // 1. Validate initial nominal temperature
    assert_eq!(tm.cooling_state(), CoolingState::Normal);
    assert!(!tm.is_throttling());

    // 2. Simulate heavy AI execution thermal spike on CPU (68°C)
    tm.update_temperature(ThermalZone::CpuJunction, 68);
    assert_eq!(tm.cooling_state(), CoolingState::Throttled);
    assert!(tm.is_throttling());
    assert_eq!(tm.throttle_delay_micros(), 20_000);

    // 3. Cool down back to nominal
    tm.update_temperature(ThermalZone::CpuJunction, 42);
    assert_eq!(tm.cooling_state(), CoolingState::Normal);
    assert!(!tm.is_throttling());

    crate::serial_println!(
        "[TEST] Dynamic Thermal Governor: Multi-zone trip points (Normal -> Throttled 68°C -> Normal 42°C) VERIFIED."
    );
}
