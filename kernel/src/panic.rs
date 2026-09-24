//! Kernel Panic Handler
//!
//! Captures fatal system errors, prints full diagnostic backtrace/location
//! information to the COM1 serial console, and enters a safe CPU halt loop.

use core::panic::PanicInfo;
use crate::serial_println;

/// Custom panic handler invoked on unexpected kernel failures or assertions.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();

    serial_println!("\n========================================================");
    serial_println!("                 INTEROS CORE KERNEL PANIC                ");
    serial_println!("========================================================");
    if let Some(location) = info.location() {
        serial_println!("Origin:  {}:{}:{}", location.file(), location.line(), location.column());
    } else {
        serial_println!("Origin:  Unknown location");
    }
    serial_println!("Details: {}", info.message());
    serial_println!("Action:  CPU halted safely to prevent hardware/state corruption.");
    serial_println!("========================================================\n");

    loop {
        x86_64::instructions::hlt();
    }
}
