//! Inter OS Native AI Assistant App
//!
//! Provides intent processing and system assistant queries in Ring 3.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[AI ASSISTANT (Ring 3)] Initialized inside Ring 3 sandbox.").ok();

    loop {
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
