//! 16550 UART Serial Port Driver (COM1)
//!
//! Provides synchronized serial console output for early boot logging,
//! kernel debugging, and headless QEMU integration testing.

use core::fmt;
use spin::Mutex;
use uart_16550::backend::PioBackend;
use uart_16550::{Config, Uart16550Tty};

/// Global spinlock-protected COM1 serial port interface.
pub static SERIAL1: Mutex<Option<Uart16550Tty<PioBackend>>> = Mutex::new(None);

/// Initializes the primary COM1 serial port (I/O port 0x3F8).
pub fn init() {
    let serial = unsafe {
        Uart16550Tty::new_port(0x3F8, Config::default())
            .expect("Failed to initialize COM1 UART port")
    };
    *SERIAL1.lock() = Some(serial);
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // Print without interrupts to avoid deadlock if an interrupt handler also logs.
    interrupts::without_interrupts(|| {
        if let Some(ref mut serial) = *SERIAL1.lock() {
            serial.write_fmt(args).expect("Printing to serial port failed");
        }
    });
}

/// Prints formatted text to the COM1 serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

/// Prints formatted text with a newline to the COM1 serial interface.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(concat!($fmt, "\n"), $($arg)*));
}
