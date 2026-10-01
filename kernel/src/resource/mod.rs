//! Unified Resource Management Subsystem
//!
//! Enforces CPU, RAM, storage, battery, and thermal quotas across system tasks.

pub mod battery;
pub mod manager;
pub mod power;
pub mod thermal;

pub use battery::{BatteryFuelGauge, ChargingStatus, BATTERY_JSON_MAX};
pub use manager::{BatteryStatus, ResourceManager, ThermalStatus, WorkloadTier};
pub use power::{DvfsGovernor, PowerManager, ThermalTrip};
pub use thermal::{CoolingState, ThermalManager, ThermalZone, THERMAL_MANAGER, THERMAL_JSON_MAX};

use spin::Mutex;
use crate::serial_println;

/// Global static instance of the Unified Resource Manager.
pub static RESOURCE_MANAGER: Mutex<ResourceManager> = Mutex::new(ResourceManager::new());

/// Global static instance of the Mobile DVFS Power & Thermal Governor.
pub static POWER_MANAGER: Mutex<PowerManager> = Mutex::new(PowerManager::new(2400));

/// Global static instance of the Mobile Battery Fuel Gauge.
pub static BATTERY_GAUGE: Mutex<BatteryFuelGauge> = Mutex::new(BatteryFuelGauge::new());

/// Initializes the Resource Manager and validates priority rules via self-tests.
pub fn init() {
    let mut rm = RESOURCE_MANAGER.lock();

    // 1. Nominal conditions test
    assert!(rm.is_tier_permitted(WorkloadTier::ForegroundUserApp));
    assert!(rm.is_tier_permitted(WorkloadTier::BackgroundSync));
    assert!(rm.is_tier_permitted(WorkloadTier::BackgroundNetwork));
    assert!(rm.is_tier_permitted(WorkloadTier::BackgroundTasks));
    serial_println!("[TEST] Resource Manager Nominal State: All workload tiers permitted.");

    // 2. Low-battery test (< 20% discharging)
    rm.update_battery(14, false);
    assert!(rm.is_tier_permitted(WorkloadTier::ForegroundUserApp));
    assert!(rm.is_tier_permitted(WorkloadTier::BackgroundSync));
    assert!(!rm.is_tier_permitted(WorkloadTier::BackgroundNetwork), "Background network must pause on low battery");
    assert!(!rm.is_tier_permitted(WorkloadTier::BackgroundTasks), "Background tasks must pause on low battery");
    serial_println!("[TEST] Resource Manager Low-Battery Policy: Background network paused at 14% battery.");

    // 3. Thermal throttling test (>= 75°C)
    rm.update_battery(85, false); // Restore battery
    rm.update_thermal(78);         // Thermal spike
    assert!(rm.thermal.throttling);
    assert!(!rm.is_tier_permitted(WorkloadTier::BackgroundNetwork), "Background network must throttle at high temperature");
    serial_println!("[TEST] Resource Manager Thermal Policy: Throttling engaged at 78°C.");

    // 4. Foreground preemption test
    rm.update_thermal(40); // Restore thermal nominal
    rm.set_foreground_active(true);
    assert!(rm.is_tier_permitted(WorkloadTier::ForegroundUserApp));
    assert!(!rm.is_tier_permitted(WorkloadTier::BackgroundTasks), "Foreground task must preempt background tasks");
    serial_println!("[TEST] Resource Manager Preemption Policy: Foreground task preempts background tasks.");

    rm.set_foreground_active(false); // Reset to nominal

    // 5. Mobile DVFS Power Governor tests.
    //
    // These exercise the governor algorithm on a local instance. Driving the
    // global POWER_MANAGER here left fabricated temperatures and a fabricated
    // 94% state of charge in the live telemetry that userspace can query.
    let mut pm = PowerManager::new(2400);
    pm.update_thermal(35);
    pm.update_battery(94, true);
    pm.set_governor(DvfsGovernor::Interactive);
    pm.on_touch_interaction();
    assert_eq!(pm.current_freq_mhz, 2400, "Touch boost must elevate to 2400 MHz");
    assert_eq!(pm.display_refresh_hz, 120, "Touch boost must maintain 120 Hz display refresh");

    pm.clear_touch_boost();
    pm.update_thermal(80); // Thermal throttling (75°C - 84°C)
    assert_eq!(pm.current_freq_mhz, 1200, "Thermal throttle must step down to 1200 MHz");
    assert_eq!(pm.display_refresh_hz, 60, "Thermal throttle must step down display to 60 Hz");

    pm.update_thermal(40); // Cool down
    pm.set_governor(DvfsGovernor::Powersave);
    assert_eq!(pm.current_freq_mhz, 600, "Powersave governor must lock to 600 MHz minimum floor");
    serial_println!("[TEST] Mobile DVFS Power Governor: Dynamic frequency scaling (2400 -> 1200 -> 600 MHz) VERIFIED.");

    // 6. Battery fuel gauge model tests, also on a local instance so the global
    // gauge keeps reporting that no fuel gauge is present.
    let mut bg = BatteryFuelGauge::new();
    bg.set_telemetry(3850, 4700, 28, ChargingStatus::ChargingAC);
    assert_eq!(bg.soc_percent, 94);
    assert_eq!(bg.voltage_mv, 3850);
    assert_eq!(bg.charging_status, ChargingStatus::ChargingAC);
    let drain_mw = bg.estimate_power_drain_mw(2400, 60);
    assert!(drain_mw > 2000, "2400 MHz + 60% brightness should draw ~2990 mW");
    serial_println!("[TEST] Battery fuel gauge model: SoC/drain arithmetic VERIFIED against a synthetic reading.");
    assert!(
        BATTERY_GAUGE.lock().soc_percent == 0,
        "the global fuel gauge must not be left reporting a state of charge with no gauge attached"
    );

    // 7. Thermal governor policy tests on a local instance.
    let mut tm = thermal::ThermalManager::new();
    tm.update_temperature(thermal::ThermalZone::CpuJunction, 78);
    assert!(tm.is_throttling(), "78 C must engage thermal throttling");
    tm.update_temperature(thermal::ThermalZone::CpuJunction, 35);
    assert!(!tm.is_throttling(), "35 C must clear thermal throttling");
    assert!(!thermal::THERMAL_MANAGER.lock().sensor_present, "the global thermal governor must not be left reporting temperatures with no sensor attached");

    serial_println!("[OK] Resource governor algorithms verified; no telemetry published without a bound sensor.");
}

