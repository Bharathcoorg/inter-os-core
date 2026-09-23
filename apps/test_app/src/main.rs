//! InterOS Core Application: test_app
//! Template: basic
#![no_std]
#![no_main]

use interos_std::ipc;
use interos_std::log;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    log::init("org.interos.test_app");
    log::info("test_app initialized inside Ring 3 sandbox");

    // Main event loop handling IPC requests from UI
    loop {
        let mut msg_buf = [0u8; 512];
        if let Ok((sender_id, msg_type, _len)) = ipc::receive_any(&mut msg_buf) {
            log::info("Received IPC packet from sender");
            let _ = ipc::send(sender_id, msg_type + 1, b"ACK");
        }
        interos_std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
