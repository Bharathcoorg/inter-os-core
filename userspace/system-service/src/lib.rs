#![no_std]

use interos_std as std;

pub fn run(service_id: u8, name: &str) -> ! {
    let _ = std::log(0, name);
    loop {
        let _ = std::service_heartbeat(service_id);
        #[cfg(feature = "ci-fault-injection")]
        if service_id == 8 {
            if let Ok(ticks) = std::get_ticks(1) {
                // CI-only one-shot crash window. Recovery backoff guarantees the
                // restarted camera service is outside this window.
                if (150..=250).contains(&ticks) {
                    std::exit_with_code(42);
                }
            }
        }
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
        let _ = std::get_ticks(1);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}
