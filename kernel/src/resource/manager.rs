// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
    pub wasm_fuel_limit_per_frame: u64,
    pub wasm_memory_quota_bytes: usize,
    pub wasm_fuel_metered_this_frame: u64,
    pub wasm_memory_allocated_bytes: usize,
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
            wasm_fuel_limit_per_frame: 500_000,
            wasm_memory_quota_bytes: 1024 * 1024,
            wasm_fuel_metered_this_frame: 0,
            wasm_memory_allocated_bytes: 0,
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

    /// Meters WASM instruction fuel against the per-frame execution limit.
    pub fn meter_wasm_instruction_fuel(&mut self, fuel: u64) -> Result<(), &'static str> {
        let new_fuel = match self.wasm_fuel_metered_this_frame.checked_add(fuel) {
            Some(val) if val <= self.wasm_fuel_limit_per_frame => val,
            _ => return Err("WasmFuelQuotaExceeded"),
        };
        self.wasm_fuel_metered_this_frame = new_fuel;
        Ok(())
    }

    /// Allocates WASM linear memory bytes against the configured quota.
    pub fn allocate_wasm_memory(&mut self, bytes: usize) -> Result<(), &'static str> {
        let new_alloc = match self.wasm_memory_allocated_bytes.checked_add(bytes) {
            Some(val) if val <= self.wasm_memory_quota_bytes => val,
            _ => return Err("WasmMemoryQuotaExceeded"),
        };
        self.wasm_memory_allocated_bytes = new_alloc;
        Ok(())
    }

    /// Releases previously allocated WASM linear memory bytes.
    pub fn free_wasm_memory(&mut self, bytes: usize) {
        self.wasm_memory_allocated_bytes = self.wasm_memory_allocated_bytes.saturating_sub(bytes);
    }

    /// Resets WASM per-frame instruction fuel metering.
    pub fn reset_wasm_frame_metering(&mut self) {
        self.wasm_fuel_metered_this_frame = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_resource_accounting_metering_and_quotas() {
        let mut rm = ResourceManager::new();
        assert_eq!(rm.wasm_fuel_limit_per_frame, 500_000);
        assert_eq!(rm.wasm_memory_quota_bytes, 1024 * 1024);
        assert_eq!(rm.wasm_fuel_metered_this_frame, 0);
        assert_eq!(rm.wasm_memory_allocated_bytes, 0);

        // Metering within limits
        assert_eq!(rm.meter_wasm_instruction_fuel(200_000), Ok(()));
        assert_eq!(rm.wasm_fuel_metered_this_frame, 200_000);
        assert_eq!(rm.meter_wasm_instruction_fuel(300_000), Ok(()));
        assert_eq!(rm.wasm_fuel_metered_this_frame, 500_000);

        // Exceeding fuel quota
        assert_eq!(
            rm.meter_wasm_instruction_fuel(1),
            Err("WasmFuelQuotaExceeded")
        );
        assert_eq!(rm.wasm_fuel_metered_this_frame, 500_000);

        // Frame reset
        rm.reset_wasm_frame_metering();
        assert_eq!(rm.wasm_fuel_metered_this_frame, 0);
        assert_eq!(rm.meter_wasm_instruction_fuel(100), Ok(()));
        assert_eq!(rm.wasm_fuel_metered_this_frame, 100);

        // Memory quota allocations
        assert_eq!(rm.allocate_wasm_memory(512 * 1024), Ok(()));
        assert_eq!(rm.wasm_memory_allocated_bytes, 512 * 1024);
        assert_eq!(rm.allocate_wasm_memory(512 * 1024), Ok(()));
        assert_eq!(rm.wasm_memory_allocated_bytes, 1024 * 1024);

        // Exceeding memory quota
        assert_eq!(
            rm.allocate_wasm_memory(1),
            Err("WasmMemoryQuotaExceeded")
        );
        assert_eq!(rm.wasm_memory_allocated_bytes, 1024 * 1024);

        // Freeing memory
        rm.free_wasm_memory(512 * 1024);
        assert_eq!(rm.wasm_memory_allocated_bytes, 512 * 1024);
        rm.free_wasm_memory(1024 * 1024); // Saturating subtraction
        assert_eq!(rm.wasm_memory_allocated_bytes, 0);
    }
}

