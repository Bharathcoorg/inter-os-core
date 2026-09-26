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

use x86_64::instructions::port::Port;

static RX_BUF: Mutex<RxState> = Mutex::new(RxState::new());

struct RxState {
    buf: [u8; 64],
    len: usize,
}

impl RxState {
    const fn new() -> Self {
        Self {
            buf: [0u8; 64],
            len: 0,
        }
    }
}

/// Polls the 16550 COM1 UART receiver and dispatches input commands.
pub fn poll_serial_rx() {
    let mut lsr = Port::<u8>::new(0x3FD);
    let mut rbr = Port::<u8>::new(0x3F8);

    let mut count = 0;
    while count < 32 {
        let status = unsafe { lsr.read() };
        if (status & 1) == 0 {
            break;
        }
        let byte = unsafe { rbr.read() };
        count += 1;

        let mut state = RX_BUF.lock();
        if byte == b'\n' || byte == b'\r' {
            if state.len > 0 {
                let len = state.len;
                state.len = 0;
                let slice = &state.buf[..len];
                handle_serial_command(slice);
            }
        } else if state.len < 63 {
            let idx = state.len;
            state.buf[idx] = byte;
            state.len += 1;
        }
    }
}

fn handle_serial_command(cmd: &[u8]) {
    let s = if let Ok(s) = core::str::from_utf8(cmd) {
        s.trim()
    } else {
        return;
    };
    if s.is_empty() {
        return;
    }

    let mut parts = s.split_ascii_whitespace();
    let verb = match parts.next() {
        Some(v) => v,
        None => return,
    };

    if verb.eq_ignore_ascii_case("TAP") || verb.eq_ignore_ascii_case("TOUCH") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<u32>(), y_str.parse::<u32>()) {
                crate::serial_println!("[COM1 INPUT] Hardware touch tap received at ({}, {}).", x, y);
                crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Down);
                crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up);
            }
        }
    } else if verb.eq_ignore_ascii_case("DOWN") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<u32>(), y_str.parse::<u32>()) {
                crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Down);
            }
        }
    } else if verb.eq_ignore_ascii_case("UP") {
        let (x, y) = match (parts.next(), parts.next()) {
            (Some(xs), Some(ys)) => (xs.parse::<u32>().unwrap_or(0), ys.parse::<u32>().unwrap_or(0)),
            _ => (0, 0),
        };
        crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up);
    } else if s.len() == 1 {
        let c = s.chars().next().unwrap();
        crate::input::post_char_event(c);
    }
}
