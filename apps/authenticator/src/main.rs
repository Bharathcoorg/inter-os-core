//! InterOS Sovereign Keystore Authenticator
#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[AUTHENTICATOR APP] Sovereign Keystore Enclave Active.").ok();

    let mut key = [0u8; 32];
    if std::keystore_derive(1, &mut key).is_ok() {
        std::log(
            0,
            "[AUTHENTICATOR APP] Hardware Keystore Slot 1 root secret derived.",
        )
        .ok();
    }

    let interest_mask = std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;
    loop {
        match std::event_poll(interest_mask, 200) {
            Ok(events) => {
                if events & std::EVENT_IPC_MSG != 0 {
                    std::log(0, "[AUTHENTICATOR APP] Processed IPC TOTP batch request.").ok();
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
