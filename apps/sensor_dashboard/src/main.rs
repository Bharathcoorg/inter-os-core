//! InterOS Core Open-Source Reference App: Sensor Dashboard
//!
//! Demonstrates Ring 3 hardware telemetry queries:
//! - Multi-zone thermal sensors via `std::thermal_query`
//! - Ambient light (lux) and IMU orientation via `std::sensor_read`

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[SENSORS] Starting InterOS Sensor Dashboard App...").ok();

    let mut t_buf = [0u8; 128];
    if let Ok(len) = std::thermal_query(&mut t_buf) {
        if len > 0 {
            std::log(
                0,
                "[SENSORS] Thermals: SoC temperature queried successfully.",
            )
            .ok();
        }
    }

    let mut s_buf = [0u8; 128];
    if let Ok(len) = std::sensor_read(&mut s_buf) {
        if len > 0 {
            std::log(
                0,
                "[SENSORS] Sensors: Ambient lux & IMU orientation read successfully.",
            )
            .ok();
        }
    }

    std::yield_now();
    std::log(0, "[SENSORS] Sensor sampling complete.").ok();
    std::exit();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Sensor dashboard error!").ok();
    std::exit();
}
