#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    interos_system_service::run(6, "[modemd] userspace service online.");
}
