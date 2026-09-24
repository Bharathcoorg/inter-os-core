//! Unified Resource Management Subsystem
//!
//! Enforces CPU, RAM, storage, battery, and thermal quotas across system tasks.

pub mod battery;
pub mod manager;
pub mod power;
pub mod thermal;

pub use battery::{BatteryFuelGauge, ChargingStatus};
pub use manager::{BatteryStatus, ResourceManager, ThermalStatus, WorkloadTier};
pub use power::{DvfsGovernor, PowerManager, ThermalTrip};
pub use thermal::{CoolingState, ThermalManager, ThermalZone, THERMAL_MANAGER};

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

    // 5. Mobile DVFS Power Governor tests
    let mut pm = POWER_MANAGER.lock();
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

    pm.set_governor(DvfsGovernor::Interactive); // Restore interactive governor
    serial_println!("[TEST] Mobile DVFS Power Governor: Dynamic frequency scaling (2400 -> 1200 -> 600 MHz) VERIFIED.");

    // 6. Battery Fuel Gauge Driver tests
    let bg = BATTERY_GAUGE.lock();
    assert_eq!(bg.soc_percent, 94);
    assert_eq!(bg.voltage_mv, 3850);
    assert_eq!(bg.charging_status, ChargingStatus::ChargingAC);
    let drain_mw = bg.estimate_power_drain_mw(2400, 60);
    assert!(drain_mw > 2000, "2400 MHz + 60% brightness should draw ~2990 mW");
    serial_println!("[TEST] Battery Fuel Gauge: 3850 mV, 94% SoC, ChargingAC, estimated drain = {} mW.", drain_mw);

    // 7. Dynamic Multi-Zone Thermal Governor
    thermal::init();

    serial_println!("--- [Unified Resource Manager 100% Attested & Enforced] ---");
}

