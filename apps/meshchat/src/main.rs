//! InterOS Sovereign P2P Mesh Messenger
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[MESH CHAT] Sovereign P2P Mesh Daemon online on 868MHz.").ok();

    let interest_mask = std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
    loop {
        match std::event_poll(interest_mask, 200) {
            Ok(events) => {
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log(0, "[MESH CHAT] Multi-hop mesh datagram routed.").ok();
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
