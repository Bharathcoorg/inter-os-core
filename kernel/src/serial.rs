//! 16550 UART Serial Port Driver (COM1)
//!
//! Provides synchronized serial console output for early boot logging,
//! kernel debugging, and headless QEMU integration testing.

use core::fmt;
use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use uart_16550::backend::PioBackend;
use uart_16550::{Config, Uart16550Tty};

/// Global spinlock-protected COM1 serial port interface.
pub static SERIAL1: Mutex<Option<Uart16550Tty<PioBackend>>> = Mutex::new(None);

struct RappTransfer {
    package_id: String,
    expected_len: usize,
    bytes: Vec<u8>,
    received: Vec<bool>,
}

static RAPP_TRANSFER: Mutex<Option<RappTransfer>> = Mutex::new(None);

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

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn handle_rapp_command(s: &str) -> bool {
    let mut parts = s.split_ascii_whitespace();
    let Some(verb) = parts.next() else { return false; };

    if verb.eq_ignore_ascii_case("RAPP_BEGIN") {
        let Some(id) = parts.next() else { return true; };
        let Some(len) = parts.next().and_then(|v| v.parse::<usize>().ok()) else { return true; };
        if id.is_empty() || id.len() > 128 || len == 0 || len > crate::fs::MAX_FILE_SIZE {
            serial_println!("[RAPP_HOST] REJECT invalid package dimensions");
            return true;
        }
        *RAPP_TRANSFER.lock() = Some(RappTransfer {
            package_id: String::from(id),
            expected_len: len,
            bytes: alloc::vec![0u8; len],
            received: alloc::vec![false; len],
        });
        serial_println!("[RAPP_HOST] READY package={} bytes={}", id, len);
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_CHUNK") {
        let Some(offset) = parts.next().and_then(|v| v.parse::<usize>().ok()) else { return true; };
        let Some(hex) = parts.next() else { return true; };
        if hex.len() == 0 || hex.len() > 48 || hex.len() % 2 != 0 { serial_println!("[RAPP_HOST] REJECT chunk"); return true; }
        let mut transfer = RAPP_TRANSFER.lock();
        let Some(ref mut state) = *transfer else { serial_println!("[RAPP_HOST] REJECT no transfer"); return true; };
        let chunk_len = hex.len() / 2;
        if offset.checked_add(chunk_len).map(|end| end <= state.expected_len) != Some(true) {
            serial_println!("[RAPP_HOST] REJECT chunk bounds");
            return true;
        }
        let raw = hex.as_bytes();
        for i in 0..chunk_len {
            let hi = hex_nibble(raw[i * 2]);
            let lo = hex_nibble(raw[i * 2 + 1]);
            let Some(hi) = hi else { serial_println!("[RAPP_HOST] REJECT hex"); return true; };
            let Some(lo) = lo else { serial_println!("[RAPP_HOST] REJECT hex"); return true; };
            state.bytes[offset + i] = (hi << 4) | lo;
            state.received[offset + i] = true;
        }
        serial_println!("[RAPP_HOST] CHUNK offset={} len={}", offset, chunk_len);
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_COMMIT") {
        let mut transfer = RAPP_TRANSFER.lock();
        let Some(state) = transfer.take() else { serial_println!("[RAPP_HOST] REJECT no transfer"); return true; };
        if state.received.iter().any(|v| !*v) {
            serial_println!("[RAPP_HOST] REJECT incomplete transfer");
            return true;
        }
        let verified = match crate::package::parse_rapp(&state.bytes) {
            Ok(manifest) if manifest.package_id == state.package_id => manifest,
            Ok(_) => { serial_println!("[RAPP_HOST] REJECT package id mismatch"); return true; }
            Err(err) => { serial_println!("[RAPP_HOST] REJECT verification failed: {:?}", err); return true; }
        };
        let path = alloc::format!("/packages/{}.rapp", verified.package_id);
        match crate::fs::VFS.lock().install_package_blob(&verified.package_id, &state.bytes) {
            Ok(_) => {
                serial_println!("[RAPP_HOST] INSTALLED id={} version={}.{}.{} path={}",
                    verified.package_id, verified.version.0, verified.version.1, verified.version.2, path);
            }
            Err(err) => serial_println!("[RAPP_HOST] REJECT persistence failed: {:?}", err),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_LAUNCH") {
        let Some(id) = parts.next() else { return true; };
        match crate::syscall::sys_app_launch(id) {
            Ok(tid) => serial_println!("[RAPP_HOST] LAUNCHED id={} task={}", id, tid.0),
            Err(err) => serial_println!("[RAPP_HOST] LAUNCH_FAILED id={} error={:?}", id, err),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_TRUST") {
        let Some(key_hex) = parts.next() else { return true; };
        let Some(publisher) = parts.next() else { return true; };
        if key_hex.len() != 64 || publisher.len() > 32 { serial_println!("[RAPP_HOST] REJECT trust key"); return true; }
        let raw = key_hex.as_bytes();
        let mut key = [0u8; 32];
        for i in 0..32 {
            let Some(hi) = hex_nibble(raw[i * 2]) else { serial_println!("[RAPP_HOST] REJECT trust hex"); return true; };
            let Some(lo) = hex_nibble(raw[i * 2 + 1]) else { serial_println!("[RAPP_HOST] REJECT trust hex"); return true; };
            key[i] = (hi << 4) | lo;
        }
        match crate::package::REVOCATION_REGISTRY.lock().trust_key(key, publisher) {
            Ok(()) => serial_println!("[RAPP_HOST] TRUSTED publisher={}", publisher),
            Err(err) => serial_println!("[RAPP_HOST] TRUST_FAILED {}", err),
        }
        return true;
    }

    false
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
    if handle_rapp_command(s) {
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
    } else if verb.eq_ignore_ascii_case("MOVE") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<u32>(), y_str.parse::<u32>()) {
                crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Move);
            }
        }
    } else if verb.eq_ignore_ascii_case("UP") {
        let (x, y) = match (parts.next(), parts.next()) {
            (Some(xs), Some(ys)) => (xs.parse::<u32>().unwrap_or(0), ys.parse::<u32>().unwrap_or(0)),
            _ => (0, 0),
        };
        crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up);
    } else if verb.eq_ignore_ascii_case("SWIPE") || verb.eq_ignore_ascii_case("DRAG") {
        if let (Some(x1_s), Some(y1_s), Some(x2_s), Some(y2_s)) = (parts.next(), parts.next(), parts.next(), parts.next()) {
            if let (Ok(x1), Ok(y1), Ok(x2), Ok(y2)) = (x1_s.parse::<u32>(), y1_s.parse::<u32>(), x2_s.parse::<u32>(), y2_s.parse::<u32>()) {
                let steps = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(5).clamp(2, 20);
                crate::serial_println!("[COM1 INPUT] Hardware touch swipe received from ({}, {}) to ({}, {}) in {} steps.", x1, y1, x2, y2, steps);
                crate::input::post_touch_event(0, x1, y1, 1000, crate::input::TouchPhase::Down);
                for i in 1..steps {
                    let cur_x = x1 as i32 + ((x2 as i32 - x1 as i32) * (i as i32)) / (steps as i32);
                    let cur_y = y1 as i32 + ((y2 as i32 - y1 as i32) * (i as i32)) / (steps as i32);
                    crate::input::post_touch_event(0, cur_x.max(0) as u32, cur_y.max(0) as u32, 1000, crate::input::TouchPhase::Move);
                }
                crate::input::post_touch_event(0, x2, y2, 0, crate::input::TouchPhase::Up);
            }
        }
    } else if s.len() == 1 {
        let c = s.chars().next().unwrap();
        crate::input::post_char_event(c);
    }
}
