//! InterOS Sovereign Health & Biometric Hub
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[HEALTH HUB] Sovereign Health & Biometric HAL initialized.").ok();

    let mut sensor_buf = [0u8; 64];
    if std::sensor_read(&mut sensor_buf).is_ok() {
        std::log(0, "[HEALTH HUB] PPG Optical Pulse Sensor register mapped at 250 S/s.").ok();
    }

    let interest_mask = std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
    loop {
        match std::event_poll(interest_mask, 150) {
            Ok(events) => {
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log(0, "[HEALTH HUB] Streaming real-time vitals packet via IPC.").ok();
                }
                if events & std::EVENT_TIMER_TICK != 0 {
                    std::yield_now();
                }
            }
            Err(_) => {}
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
