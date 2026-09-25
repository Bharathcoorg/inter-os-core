#![no_std]

use interos_std as std;

pub fn run(service_id: u8, name: &str) -> ! {
    let _ = std::log(0, name);
    loop {
        let _ = std::service_heartbeat(service_id);
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
        let _ = std::get_ticks(2);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}
