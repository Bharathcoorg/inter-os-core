//! InterOS Core Open-Source Reference App: Hello World
//!
//! Minimal Ring 3 (User Mode) application demonstrating:
//! - Entry point `_start`
//! - Syscall logging via `interos_std::log`
//! - CPU cooperative multitasking via `interos_std::yield_now`
//! - Clean exit via `interos_std::exit`

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[HELLO WORLD] Welcome to InterOS Core (Pure Rust Mobile Microkernel)!").ok();
    
    // Yield CPU back to scheduler
    std::yield_now();

    std::log(0, "[HELLO WORLD] Task finished successfully. Exiting Ring 3 process.").ok();
    std::exit();
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Hello World app encountered an unrecoverable error!").ok();
    std::exit();
}
