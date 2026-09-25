//! Inter OS userspace PID 1 / service-manager bootstrap.
//!
//! PID 1 deliberately contains no hardware-policy demos or application logic.
//! The kernel owns only the minimum primitives; this process owns userspace
//! boot ordering, service observation, and the transition to the normal event loop.

#![no_std]
#![no_main]

use interos_std as std;
use core::panic::PanicInfo;

const SERVICE_BUFFER: usize = 512;
const POLL_INTERVAL_TICKS: u64 = 100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[PID1] Inter OS userspace init started in Ring 3.").ok();
    // Phase B service-manager bootstrap. PID 1, not kernel_main, owns the
    // startup ordering of the userspace service set.
    const SERVICES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    for service_id in SERVICES {
        match std::service_start(service_id) {
            Ok(task_id) => std::log(0, "[PID1] Service started through lifecycle ABI.").ok(),
            Err(_) => std::log(0, "[PID1] Service start request rejected.").ok(),
        }
    }


    // Establish the first userspace service-manager contract: PID 1 observes
    // the authoritative kernel service registry rather than hard-coding task IDs.
    let mut service_buf = [0u8; SERVICE_BUFFER];
    let mut last_poll = 0u64;

    loop {
        let now = std::get_ticks(0).unwrap_or(last_poll);

        if now.saturating_sub(last_poll) >= POLL_INTERVAL_TICKS || last_poll == 0 {
            if let Ok(len) = std::service_list(&mut service_buf) {
                if len > 0 {
                    std::log(0, "[PID1] Service registry observed; userspace lifecycle loop active.").ok();
                } else {
                    std::log(0, "[PID1] Service registry returned no active services.").ok();
                }
            }
            last_poll = now;
        }

        // Block cooperatively until an IPC/timer/input event instead of
        // continuously spinning in Ring 3.
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
