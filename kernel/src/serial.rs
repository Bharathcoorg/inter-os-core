// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! 16550 UART Serial Port Driver (COM1)
//!
//! Provides synchronized serial console output for early boot logging,
//! kernel debugging, and headless QEMU integration testing.

use core::fmt;
use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use uart_16550::backend::PioBackend;
use uart_16550::spec::registers::{FifoTriggerLevel, IER};
use uart_16550::{Config, Uart16550};
use crate::drivers::display::DisplayDriver;
use crate::storage::block::BlockDevice;

/// Serial port wrapper implementing `core::fmt::Write` with terminal newline normalization.
pub struct SerialPort(Uart16550<PioBackend>);

/// How many transmit-capacity polls a single byte may cost before it is
/// dropped.
///
/// WHY bounded: `Uart16550::send_bytes_exact` is documented as spinning
/// indefinitely. If the host stops draining COM1 -- QEMU's TCP chardev applies
/// backpressure when its peer pauses, and the Studio's collector briefly stops
/// reading while it persists each line -- the kernel used to wedge permanently
/// inside a `serial_println!`. Diagnostics are droppable; the CPU is not. Once a
/// byte times out the rest of that line is dropped too, so a dead console costs
/// one bounded wait per print rather than one per character.
const TX_POLL_BUDGET: usize = 10_000;

impl SerialPort {
    /// Writes one byte, or reports that the UART never became ready.
    fn write_byte(&mut self, byte: u8) -> bool {
        let mut pending: &[u8] = &[byte];
        let mut polls = 0usize;
        while !pending.is_empty() {
            let written = self.0.send_bytes(pending);
            if written > 0 {
                pending = &pending[written..];
                polls = 0;
            } else if polls >= TX_POLL_BUDGET {
                return false;
            } else {
                polls += 1;
                core::hint::spin_loop();
            }
        }
        true
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &byte in s.as_bytes() {
            let sent = if byte == b'\n' {
                self.write_byte(b'\r') && self.write_byte(byte)
            } else {
                self.write_byte(byte)
            };
            if !sent {
                break;
            }
        }
        Ok(())
    }
}

/// Global spinlock-protected COM1 serial port interface.
pub static SERIAL1: Mutex<Option<SerialPort>> = Mutex::new(None);

struct RappTransfer {
    package_id: String,
    expected_len: usize,
    bytes: Vec<u8>,
    received: Vec<bool>,
}

static RAPP_TRANSFER: Mutex<Option<RappTransfer>> = Mutex::new(None);

/// A trust anchor arriving over more console lines than one line can hold.
///
/// WHY a bitmap rather than a running offset: the console is a queue, so the same
/// chunk can arrive twice -- a person re-sending a line whose acknowledgement
/// scrolled past. Rewriting the same bytes at the same position is harmless,
/// while an offset that trusted line order would slide the anchor and store a
/// certificate that is neither the original nor anything recognisable.
struct AnchorTransfer {
    expected_len: usize,
    der: [u8; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES],
    received: [bool; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES],
}

static ANCHOR_TRANSFER: Mutex<Option<AnchorTransfer>> = Mutex::new(None);

/// The most hex one `TLS_TRUST_CHUNK` line may carry.
///
/// WHY 240 hex characters, or 120 bytes: a console line accumulates up to 511
/// characters and one past that discards the whole line, so a chunk leaves room
/// for the verb and a four-digit offset. It is also well under a sixth of the
/// largest anchor, so even a full 2 KiB certificate arrives in eighteen lines.
const ANCHOR_CHUNK_HEX_CHARS: usize = 240;

/// Initializes the primary COM1 serial port (I/O port 0x3F8).
///
/// Avoids `test_loopback()` which panics if external TCP or terminal clients
/// transmit bytes into COM1 during machine startup or reset.
///
/// WHY the data-ready interrupt is raised rather than left at the default:
/// `Config::DEFAULT` writes `IER = 0`, so COM1 never asserts IRQ4 and an
/// arriving touch byte wakes nothing of its own. A CPU parked in
/// `enable_and_hlt` then resumes only on the next timer tick, which put a whole
/// tick between a person's tap and the kernel ever looking at it.
///
/// WHY the one-byte FIFO trigger: at the default fourteen the chip holds a
/// short command -- `TAP 100 650` is eleven bytes -- back until its character
/// timeout, so arrival would still be reported late with IRQ4 open. The trigger
/// costs one interrupt per byte instead of one per line, and the handler drains
/// the FIFO in the same pass, so a burst cannot overrun it into silence.
pub fn init() {
    let mut port = match unsafe { Uart16550::new_port(0x3F8) } {
        Ok(p) => p,
        Err(_) => return,
    };
    let config = Config {
        interrupts: IER::DATA_READY,
        fifo_trigger_level: Some(FifoTriggerLevel::One),
        ..Config::DEFAULT
    };
    let _ = port.init(config);
    *SERIAL1.lock() = Some(SerialPort(port));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = _print_internal(args);
}

/// Internal helper for printing formatted text to COM1.
///
/// Uses bounded `try_lock` attempts so panic handlers and interrupt paths
/// never deadlock if `SERIAL1` was held when a fault occurred.
/// Inside the locked region, `write_fmt` errors are safely discarded instead
/// of panicking via `.expect(...)`.
///
/// WHY interrupts stay on: this used to run inside `with_interrupts_disabled`,
/// whose stated purpose was to stop an interrupt handler deadlocking against the
/// same spinlock. The COM1 handler does not take this lock at all --
/// `poll_serial_rx` reads the raw receiver port and takes `RX_BUF` -- so the
/// mask never protected anything, and it made every console line an
/// interrupt-off window: a single `DIAG_STATE` reply held the timer and a
/// arriving tap out for as long as its ~230 bytes took to leave the port. The
/// bounded `try_lock` is what actually prevents the deadlock, and a handler that
/// cannot get the lock drops its line instead of spinning.
pub fn _print_internal(args: fmt::Arguments) -> bool {
    use core::fmt::Write;

    for _ in 0..10_000 {
        if let Some(mut guard) = SERIAL1.try_lock() {
            if let Some(ref mut serial) = *guard {
                let _ = serial.write_fmt(args);
            }
            return true;
        }
        core::hint::spin_loop();
    }
    false
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

#[cfg(not(test))]
use x86_64::instructions::port::Port;

/// Physical hardware killswitch states for sensors, comms, and peripherals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareKillswitches {
    pub camera_cut: bool,
    pub mic_cut: bool,
    pub cellular_cut: bool,
    pub gps_cut: bool,
}

impl HardwareKillswitches {
    pub const fn new() -> Self {
        Self {
            camera_cut: false,
            mic_cut: false,
            cellular_cut: false,
            gps_cut: false,
        }
    }
}

pub static HARDWARE_KILLSWITCHES: Mutex<HardwareKillswitches> = Mutex::new(HardwareKillswitches::new());

static RX_BUF: Mutex<RxState> = Mutex::new(RxState::new());

/// Number of completed command lines that can be queued between the COM1
/// interrupt and the main loop's dispatch pass.
///
/// WHY a queue rather than a single buffer: the interrupt handler must not
/// dispatch, so completed lines have to be held somewhere. With a single buffer
/// a second line arriving before the main loop runs would be appended to the
/// first and the two would execute as one garbled command. Four slots is ample
/// for a human or a test harness, and the overflow is reported rather than
/// silently dropping the oldest.
const RX_QUEUE_SLOTS: usize = 32;

#[cfg_attr(test, allow(dead_code))]
struct RxState {
    /// Line currently being accumulated.
    buf: [u8; 512],
    len: usize,
    /// Set when the current line exceeded the buffer.
    overflowed: bool,
    /// Completed lines awaiting dispatch.
    queue: [[u8; 512]; RX_QUEUE_SLOTS],
    qlens: [usize; RX_QUEUE_SLOTS],
    qhead: usize,
    qtail: usize,
    qcount: usize,
    /// Completed lines dropped because the queue was full.
    dropped: u32,
    /// The tick each queued line became complete, indexed alongside `queue`.
    qpush_ticks: [u64; RX_QUEUE_SLOTS],
    /// How long the line most recently handed to `handle_serial_command` had
    /// already waited, the worst such wait since boot, and how many lines have
    /// been dispatched. All three are in `SYSTEM_TICKS` units, which are
    /// milliseconds.
    ///
    /// WHY measured at all: a byte reaching this queue is not the event a person
    /// is waiting on -- dispatch only happens where `service_serial_commands`
    /// is called, so the queue can hold a finished command while nothing looks.
    /// Without the wait in the open, that stall is indistinguishable from a slow
    /// handler or a slow app.
    last_wait_ticks: u64,
    max_wait_ticks: u64,
    dispatched: u64,
}

#[cfg_attr(test, allow(dead_code))]
impl RxState {
    const fn new() -> Self {
        Self {
            buf: [0u8; 512],
            len: 0,
            overflowed: false,
            queue: [[0u8; 512]; RX_QUEUE_SLOTS],
            qlens: [0; RX_QUEUE_SLOTS],
            qhead: 0,
            qtail: 0,
            qcount: 0,
            dropped: 0,
            qpush_ticks: [0; RX_QUEUE_SLOTS],
            last_wait_ticks: 0,
            max_wait_ticks: 0,
            dispatched: 0,
        }
    }

    /// Pushes the accumulated line onto the queue. Returns false if it was dropped.
    fn push_current_line(&mut self) -> bool {
        if self.qcount >= RX_QUEUE_SLOTS {
            self.dropped += 1;
            return false;
        }
        let len = self.len;
        self.queue[self.qtail][..len].copy_from_slice(&self.buf[..len]);
        self.qlens[self.qtail] = len;
        self.qpush_ticks[self.qtail] = crate::interrupts::SYSTEM_TICKS
            .load(core::sync::atomic::Ordering::Relaxed);
        self.qtail = (self.qtail + 1) % RX_QUEUE_SLOTS;
        self.qcount += 1;
        true
    }

    /// Pops the oldest completed line into `out`, returning its length.
    fn pop_line(&mut self, out: &mut [u8; 512]) -> usize {
        if self.qcount == 0 {
            return 0;
        }
        let len = self.qlens[self.qhead];
        let now = crate::interrupts::SYSTEM_TICKS.load(core::sync::atomic::Ordering::Relaxed);
        let waited = now.saturating_sub(self.qpush_ticks[self.qhead]);
        self.last_wait_ticks = waited;
        if waited > self.max_wait_ticks {
            self.max_wait_ticks = waited;
        }
        self.dispatched += 1;
        out[..len].copy_from_slice(&self.queue[self.qhead][..len]);
        self.qhead = (self.qhead + 1) % RX_QUEUE_SLOTS;
        self.qcount -= 1;
        len
    }

    /// The queue-wait readings the diagnostics line publishes.
    fn wait_stats(&self) -> (u64, u64, u64) {
        (self.last_wait_ticks, self.max_wait_ticks, self.dispatched)
    }
}

/// Polls the 16550 COM1 UART receiver and dispatches input commands.
/// Drains the UART receive FIFO into the line buffer.
///
/// WHY this no longer dispatches commands: this runs in the COM1 interrupt
/// handler (`irq4_com1_handler`), and `handle_serial_command` launches apps,
/// renders surfaces and prints. Doing that in interrupt context is wrong twice
/// over. It can block on a lock the interrupted code already holds — the
/// scheduler, `DRM_KMS`, `SCHEDULER` — which is a deadlock, not a slow path. And
/// every `serial_println!` it performs writes into a 16-byte hardware FIFO from
/// inside the handler, so a response long enough to fill it loses bytes.
///
/// The handler now only accumulates bytes atomically under `without_interrupts`.
/// Dispatch happens in `service_serial_commands`, called from the main loop.
pub fn poll_serial_rx() {
    #[cfg(not(test))]
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut lsr = Port::<u8>::new(0x3FD);
        let mut rbr = Port::<u8>::new(0x3F8);

        let status = unsafe { lsr.read() };
        if (status & 1) == 0 {
            return;
        }

        let mut state = RX_BUF.lock();
        let mut cur_status = status;
        let mut count = 0;
        while count < 128 {
            if (cur_status & 1) == 0 {
                break;
            }
            let byte = unsafe { rbr.read() };
            count += 1;

            if byte != 0 {
                if byte == b'\n' || byte == b'\r' {
                    if state.overflowed {
                        state.len = 0;
                        state.overflowed = false;
                    } else if state.len > 0 {
                        state.push_current_line();
                        state.len = 0;
                    }
                } else if state.len < 511 {
                    let idx = state.len;
                    state.buf[idx] = byte;
                    state.len += 1;
                } else {
                    state.overflowed = true;
                }
            }
            cur_status = unsafe { lsr.read() };
        }
    });
}

/// Returns true if there is unread data in the serial RX queue or line buffer.
pub fn has_rx_data() -> bool {
    if let Some(state) = RX_BUF.try_lock() {
        state.qcount > 0 || state.len > 0
    } else {
        false
    }
}

/// Dispatches any complete serial command lines. Must be called from a normal
/// (non-interrupt) context.
pub fn service_serial_commands() {
    poll_serial_rx();
    loop {
        // Copy the line out and release the lock before dispatching:
        // handle_serial_command prints, and printing while holding RX_BUF would
        // block the interrupt handler that fills it.
        let mut local = [0u8; 512];
        let (len, dropped) = {
            #[cfg(not(test))]
            let res = x86_64::instructions::interrupts::without_interrupts(|| {
                let mut state = RX_BUF.lock();
                let dropped = state.dropped;
                state.dropped = 0;
                (state.pop_line(&mut local), dropped)
            });
            #[cfg(test)]
            let res = {
                let mut state = RX_BUF.lock();
                let dropped = state.dropped;
                state.dropped = 0;
                (state.pop_line(&mut local), dropped)
            };
            res
        };
        if dropped > 0 {
            serial_println!(
                "[SERIAL] {} command line(s) dropped: the {}-slot receive queue was full.",
                dropped,
                RX_QUEUE_SLOTS
            );
        }
        if len == 0 {
            return;
        }

        let mut start = 0usize;
        let mut end = len;
        while start < end && ((local[start] as char).is_ascii_whitespace() || local[start] == 0) {
            start += 1;
        }
        while end > start && ((local[end - 1] as char).is_ascii_whitespace() || local[end - 1] == 0) {
            end -= 1;
        }
        if start == end {
            continue;
        }
        handle_serial_command(&local[start..end]);
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

/// A file the host is writing into the guest.
///
/// WHY the state is here and not in the caller: the console carries one
/// write at a time, and the chunks of a file arrive as separate
/// commands with nothing tying them together but the path that named
/// them.
struct GuestWrite {
    path: String,
    expected_len: usize,
    bytes: Vec<u8>,
}

static GUEST_WRITE: Mutex<Option<GuestWrite>> = Mutex::new(None);

/// Whether a path may be reached from the console.
///
/// WHY the guest decides and not the host: the host's check is a
/// convenience, and a convenience is not an authority. The console is
/// outside the capability model, so the path is checked here, against
/// the same shape the host uses, before anything is read or written.
fn guest_path_is_safe(path: &str, writable: bool) -> bool {
    if path.is_empty() || path.len() > 192 {
        return false;
    }
    if !path.starts_with('/') || path.contains("//") || path.contains("..") {
        return false;
    }
    if path
        .bytes()
        .any(|b| b < 0x20 || matches!(b, b'|' | b';' | b'&' | b'$' | b'\'' | b'"' | b'\\'))
    {
        return false;
    }
    // WHY writes reach the media directories too: a Studio screenshot or a
    // host-delivered download belongs where the Gallery and File Manager
    // list documents. /data stays the platform's own area; media storage is
    // the user's. Anything else is refused so the console cannot place a
    // file among the platform's packages.
    if writable
        && !path.starts_with("/data/")
        && !path.starts_with("/storage/photos/")
        && !path.starts_with("/storage/pictures/")
        && !path.starts_with("/storage/videos/")
    {
        return false;
    }
    true
}

/// The guest's side of the Studio's filesystem channel.
///
/// WHY this exists: the Studio could install packages and read the
/// device's own catalogue, but the guest's files were unreachable in
/// both directions. A model could never be put on the device, and a file
/// the guest wrote could never be read from the host. The console is the
/// only channel between the two, so the console carries the files.
fn handle_guest_fs_command(parts: &mut core::str::SplitAsciiWhitespace<'_>) -> bool {
    let Some(verb) = parts.next() else { return false };

    if verb.eq_ignore_ascii_case("STUDIO_FS_LIST") {
        let Some(path) = parts.next() else {
            serial_println!("[STUDIO_FS] ERROR missing path");
            return true;
        };
        if !guest_path_is_safe(path, false) {
            serial_println!("[STUDIO_FS] ERROR unsafe path");
            return true;
        }
        let mut buf = [0u8; 4096];
        let listed = {
            let vfs = crate::fs::VFS.lock();
            vfs.list_dir_by_path(path, &mut buf)
        };
        match listed {
            Ok(n) => {
                serial_println!("LIST_OK path={}", path);
                for line in buf[..n].split(|b| *b == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(text) = core::str::from_utf8(line) else { continue };
                    let mut fields = text.split('|');
                    let (Some(name), Some(kind), Some(size)) =
                        (fields.next(), fields.next(), fields.next())
                    else {
                        continue;
                    };
                    serial_println!("[STUDIO_FS_ENTRY] path={}|{}|{}", name, kind, size);
                }
            }
            Err(_) => {
                serial_println!("[STUDIO_FS] ERROR list failed path={}", path);
            }
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("STUDIO_FS_READ") {
        let Some(path) = parts.next() else {
            serial_println!("[STUDIO_FS] ERROR missing path");
            return true;
        };
        let offset = parts.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        let length = parts.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(192);
        if !guest_path_is_safe(path, false) {
            serial_println!("[STUDIO_FS] ERROR unsafe path");
            return true;
        }
        // WHY the read is bounded: the reply is a line on the console, and
        // a line that is longer than the console's buffer is a line the
        // host never sees whole.
        let length = length.clamp(1, 192);
        let mut buf = [0u8; 192];
        let read = {
            let vfs = crate::fs::VFS.lock();
            vfs.read_file_range(path, offset, &mut buf[..length])
        };
        match read {
            Ok(n) => {
                let mut hex = [0u8; 384];
                let mut hex_len = 0usize;
                for byte in &buf[..n] {
                    hex[hex_len] = b"0123456789abcdef"[(byte >> 4) as usize];
                    hex[hex_len + 1] = b"0123456789abcdef"[(byte & 0x0f) as usize];
                    hex_len += 2;
                }
                serial_println!(
                    "[STUDIO_FS_READ] hex={} len={}",
                    core::str::from_utf8(&hex[..hex_len]).unwrap_or(""),
                    n
                );
            }
            Err(_) => {
                serial_println!("[STUDIO_FS] ERROR read failed path={}", path);
            }
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("STUDIO_FS_BEGIN") {
        let Some(path) = parts.next() else {
            serial_println!("[STUDIO_FS] ERROR missing path");
            return true;
        };
        let len = parts.next().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
        if !guest_path_is_safe(path, true) {
            serial_println!("[STUDIO_FS] ERROR unsafe path");
            return true;
        }
        if len == 0 || len > crate::fs::vfs::MAX_FILE_SIZE {
            serial_println!("[STUDIO_FS] ERROR invalid length {}", len);
            return true;
        }
        *GUEST_WRITE.lock() = Some(GuestWrite {
            path: String::from(path),
            expected_len: len,
            bytes: alloc::vec![0u8; len],
        });
        serial_println!("WRITE_READY path={}", path);
        return true;
    }

    if verb.eq_ignore_ascii_case("STUDIO_FS_CHUNK") {
        let Some(offset) = parts.next().and_then(|v| v.parse::<usize>().ok()) else {
            serial_println!("[STUDIO_FS] ERROR missing offset");
            return true;
        };
        let Some(hex) = parts.next() else {
            serial_println!("[STUDIO_FS] ERROR missing chunk");
            return true;
        };
        if hex.len() == 0 || hex.len() > 512 || hex.len() % 2 != 0 {
            serial_println!("[STUDIO_FS] ERROR invalid chunk");
            return true;
        }
        let chunk_len = hex.len() / 2;
        {
            let mut write = GUEST_WRITE.lock();
            let Some(ref mut state) = *write else {
                serial_println!("[STUDIO_FS] ERROR no write in progress");
                return true;
            };
            if offset.checked_add(chunk_len).map(|end| end <= state.expected_len) != Some(true) {
                serial_println!("[STUDIO_FS] ERROR chunk out of bounds");
                return true;
            }
            let raw = hex.as_bytes();
            for i in 0..chunk_len {
                let Some(hi) = hex_nibble(raw[i * 2]) else {
                    serial_println!("[STUDIO_FS] ERROR bad hex");
                    return true;
                };
                let Some(lo) = hex_nibble(raw[i * 2 + 1]) else {
                    serial_println!("[STUDIO_FS] ERROR bad hex");
                    return true;
                };
                state.bytes[offset + i] = (hi << 4) | lo;
            }
        }
        // WHY the guard is released first: this acknowledgement is the only
        // serial output the host waits for per chunk, and holding the write
        // lock across it would trade a console stall for a filesystem stall.
        serial_println!("CHUNK_OK offset={}", offset);
        return true;
    }

    if verb.eq_ignore_ascii_case("STUDIO_FS_COMMIT") {
        let mut write = GUEST_WRITE.lock();
        let Some(state) = write.take() else {
            serial_println!("[STUDIO_FS] ERROR no write in progress");
            return true;
        };
        let path = state.path.clone();
        let bytes = state.bytes;
        let written = {
            let mut vfs = crate::fs::VFS.lock();
            // WHY the parents are made first: a file written into a
            // directory the guest has never seen would be a file nobody
            // can list, and the host would believe it had been written.
            let mut parent = String::new();
            let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            for part in &parts[..parts.len().saturating_sub(1)] {
                if !parent.is_empty() {
                    parent.push('/');
                }
                parent.push_str(part);
                let _ = vfs.mkdir(&parent);
            }
            vfs.create_file(&path, &bytes, false);
            vfs.sync()
        };
        match written {
            Ok(()) => serial_println!("WRITE_OK path={}", path),
            Err(_) => serial_println!("[STUDIO_FS] ERROR commit failed path={}", path),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("STUDIO_FS_DELETE_FILE") {
        let Some(path) = parts.next() else {
            serial_println!("[STUDIO_FS] ERROR missing path");
            return true;
        };
        if !guest_path_is_safe(path, true) {
            serial_println!("[STUDIO_FS] ERROR unsafe path");
            return true;
        }
        let removed = {
            let mut vfs = crate::fs::VFS.lock();
            vfs.unlink(path)
        };
        match removed {
            Ok(()) => serial_println!("DELETE_OK path={}", path),
            Err(_) => serial_println!("[STUDIO_FS] ERROR delete failed path={}", path),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("STUDIO_FS_MKDIR") {
        let Some(path) = parts.next() else {
            serial_println!("[STUDIO_FS] ERROR missing path");
            return true;
        };
        if !guest_path_is_safe(path, true) {
            serial_println!("[STUDIO_FS] ERROR unsafe path");
            return true;
        }
        {
            let mut vfs = crate::fs::VFS.lock();
            vfs.mkdir(path);
        }
        serial_println!("MKDIR_OK path={}", path);
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_UNINSTALL") {
        let Some(package_id) = parts.next() else {
            serial_println!("[STUDIO_PKG] ERROR missing package id");
            return true;
        };
        let removed = {
            let mut vfs = crate::fs::VFS.lock();
            vfs.remove_app_storage(package_id)
        };
        match removed {
            Ok(()) => serial_println!("UNINSTALL_OK package={}", package_id),
            Err(_) => serial_println!("[STUDIO_PKG] ERROR uninstall failed package={}", package_id),
        }
        return true;
    }

    false
}

fn handle_rapp_command(s: &str) -> bool {
    let mut parts = s.split_ascii_whitespace();
    let Some(verb) = parts.next() else { return false; };

    if verb.eq_ignore_ascii_case("RAPP_BEGIN") {
        let Some(id) = parts.next() else { return true; };
        let Some(len) = parts.next().and_then(|v| v.parse::<usize>().ok()) else { return true; };
        if id.is_empty() || id.len() > 128 || len == 0 || len > crate::package::store::MAX_BLOB_SIZE {
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
        let previous_entry = {
            let registry = crate::package::PACKAGE_REGISTRY.lock();
            registry.entries().iter().find(|e| e.package_id == verified.package_id).cloned()
        };
        let previous_blob = {
            let store = crate::package::PACKAGE_BLOB_STORE.lock();
            store.get(&verified.package_id).cloned()
        };
        if crate::package::PACKAGE_BLOB_STORE.lock().put(&verified.package_id, verified.version, &state.bytes).is_err() {
            serial_println!("[RAPP_HOST] REJECT package store persistence failed");
            return true;
        }
        let registry_ok = {
            let mut registry = crate::package::PACKAGE_REGISTRY.lock();
            registry.stage_verified(&verified).is_ok() && registry.activate(&verified.package_id).is_ok()
        };
        if registry_ok {
            serial_println!("[RAPP_HOST] INSTALLED id={} version={}.{}.{}",
                verified.package_id, verified.version.0, verified.version.1, verified.version.2);
        } else {
            if let Some(previous) = previous_blob {
                let _ = crate::package::PACKAGE_BLOB_STORE.lock().put(&previous.package_id, previous.version, &previous.bytes);
            } else {
                let _ = crate::package::PACKAGE_BLOB_STORE.lock().remove(&verified.package_id);
            }
            if let Some(previous) = previous_entry {
                let _ = crate::package::PACKAGE_REGISTRY.lock().restore_entry(previous);
            } else {
                let _ = crate::package::PACKAGE_REGISTRY.lock().rollback_install(&verified.package_id, None);
            }
            serial_println!("[RAPP_HOST] REJECT registry activation failed");
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_LAUNCH") {
        let Some(id) = parts.next() else { return true; };
        match crate::syscall::dev_launch_verified_rapp(id) {
            Ok(tid) => serial_println!("[RAPP_HOST] LAUNCHED id={} task={}", id, tid.0),
            Err(err) => serial_println!("[RAPP_HOST] LAUNCH_FAILED id={} error={:?}", id, err),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("APP_LAUNCH") {
        let Some(id) = parts.next() else { return true; };
        match crate::syscall::dev_launch_app(id) {
            Ok(tid) => serial_println!("[RAPP_HOST] LAUNCHED id={} task={}", id, tid.0),
            Err(err) => serial_println!("[RAPP_HOST] LAUNCH_FAILED id={} error={:?}", id, err),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_UNINSTALL") {
        let Some(id) = parts.next() else { return true; };
        match crate::syscall::dev_uninstall_verified_rapp(id) {
            Ok(()) => serial_println!("[RAPP_HOST] UNINSTALLED id={}", id),
            Err(err) => serial_println!("[RAPP_HOST] UNINSTALL_FAILED id={} error={:?}", id, err),
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("INPUT_TOUCH") {
        let phase = match parts.next().map(|v| v.to_ascii_uppercase()).as_deref() {
            Some("DOWN") => crate::input::TouchPhase::Down,
            Some("MOVE") => crate::input::TouchPhase::Move,
            Some("UP") => crate::input::TouchPhase::Up,
            Some("CANCEL") => crate::input::TouchPhase::Cancel,
            _ => { serial_println!("[COM1 INPUT] REJECT invalid touch phase"); return true; }
        };
        let Some(px) = parts.next().and_then(|v| v.parse::<u32>().ok()) else { return true; };
        let Some(py) = parts.next().and_then(|v| v.parse::<u32>().ok()) else { return true; };
        let Some(slot) = parts.next().and_then(|v| v.parse::<u8>().ok()) else { return true; };
        // INPUT_TOUCH carries logical portrait coordinates (px, py), the same
        // CoordinateSpace the compositor, window manager and apps use. Mapping
        // the guest's letterboxed scanout back into UI space here would duplicate
        // the presentation geometry in a second place with different constants,
        // which previously rejected every gesture above the landscape midpoint.
        const LOGICAL_WIDTH: u32 = 720;
        const LOGICAL_HEIGHT: u32 = 1440;
        if px >= LOGICAL_WIDTH || py >= LOGICAL_HEIGHT || slot >= crate::input::MAX_TOUCH_SLOTS as u8 {
            serial_println!(
                "[COM1 INPUT] REJECT touch bounds x={} y={} (logical {}x{})",
                px, py, LOGICAL_WIDTH, LOGICAL_HEIGHT
            );
            return true;
        }
        crate::input::post_touch_event(slot, px, py, if phase == crate::input::TouchPhase::Up { 0 } else { 512 }, phase);
        // WHY an accepted touch is not printed: a drag is twenty to forty of these
        // lines, each one a byte-poked transmit that used to hold interrupts off
        // for its whole length, all to say nothing a host could not act on. The
        // rejections above stay, because those are the answers that change what the
        // sender does next. The tap path keeps its own marker
        // (`Hardware touch tap received`) for the latency bench to correlate on.
        return true;
    }

    if verb.eq_ignore_ascii_case("INPUT_KEY") {
        let Some(code) = parts.next().and_then(|v| v.parse::<u32>().ok()) else { return true; };
        if code > 0x10FFFF { serial_println!("[COM1 INPUT] REJECT key code"); return true; }
        if let Some(c) = core::char::from_u32(code) {
            crate::input::post_char_event(c);
            serial_println!("[COM1 INPUT] KEY code={} char={:?}", code, c);
        }
        return true;
    }

    // WHY this is the only key-name table: handle_serial_command asks this handler
    // before its own chain and returns whenever it answers, so a second INPUT_KEYCODE
    // branch written further down that chain is never reached. The one that used to
    // live there named Enter and Backspace the wrong way round.
    if verb.eq_ignore_ascii_case("INPUT_KEYCODE") {
        let Some(name) = parts.next() else { return true; };
        let code = match name.to_ascii_uppercase().as_str() {
            "ENTER" => crate::input::KeyCode::Enter,
            "ESCAPE" | "ESC" => crate::input::KeyCode::Escape,
            "BACKSPACE" => crate::input::KeyCode::Backspace,
            "TAB" => crate::input::KeyCode::Tab,
            "ARROWUP" | "UP" => crate::input::KeyCode::ArrowUp,
            "ARROWDOWN" | "DOWN" => crate::input::KeyCode::ArrowDown,
            "ARROWLEFT" | "LEFT" => crate::input::KeyCode::ArrowLeft,
            "ARROWRIGHT" | "RIGHT" => crate::input::KeyCode::ArrowRight,
            "HOME" => crate::input::KeyCode::Home,
            "END" => crate::input::KeyCode::End,
            "PAGEUP" => crate::input::KeyCode::PageUp,
            "PAGEDOWN" => crate::input::KeyCode::PageDown,
            _ => { serial_println!("[COM1 INPUT] REJECT key code"); return true; }
        };
        crate::input::post_key_event(crate::input::KeyEvent {
            source: crate::input::InputSource::DeviceStudio,
            action: crate::input::KeyAction::Down,
            code,
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        });
        serial_println!("[COM1 INPUT] KEYCODE {:?}", code);
        return true;
    }

    if verb.eq_ignore_ascii_case("NAV") {
        let action = parts.next().map(|v| v.to_ascii_uppercase());
        let kind = match action.as_deref() {
            Some("HOME") => Some(crate::input::GestureKind::Home),
            Some("BACK") => Some(crate::input::GestureKind::Back),
            Some("RECENTS") | Some("OVERVIEW") => Some(crate::input::GestureKind::Recents),
            _ => None,
        };
        if let Some(kind) = kind {
            let ev = crate::drivers::input::InputEvent::from_canonical(crate::input::GestureEvent {
                kind,
                position: crate::input::Point { x: 360, y: 1400 },
                delta_x: 0,
                delta_y: 0,
                pointer_id: 0,
                scale_milli: 1000,
            });
            crate::drivers::input::dispatch_to_focused_window(ev);
            crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
            serial_println!("[COM1 INPUT] NAV {:?}", kind);
        } else {
            serial_println!("[COM1 INPUT] REJECT unknown NAV action");
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("ORIENTATION") {
        let opt = parts.next().map(|v| v.to_ascii_uppercase());
        let set_kms_orientation = |orient: crate::drivers::display_kms::DisplayOrientation,
                                    phys_w: u16,
                                    phys_h: u16| {
            let mut applied = false;
            for _ in 0..100 {
                if let Some(mut kms) = crate::drivers::display_kms::DRM_KMS.try_lock() {
                    kms.set_orientation(orient);
                    drop(kms);
                    applied = true;
                    break;
                }
                core::hint::spin_loop();
            }
            if !applied {
                return false;
            }
            // Keep the physical scanout in lockstep with the KMS logical
            // scanout: present_argb8888 then blits 1:1 in both orientations
            // (720x1440 portrait, 1440x720 landscape) with no resample.
            // Called after the KMS guard is dropped so the two locks are
            // never held together.
            if !crate::framebuffer::set_physical_mode(phys_w, phys_h) {
                serial_println!(
                    "[DISPLAY_HAL] WARN: physical scanout {}x{} rejected; staying on previous mode",
                    phys_w,
                    phys_h
                );
            }
            true
        };
        match opt.as_deref() {
            Some("LANDSCAPE") | Some("LANDSCAPE_RIGHT") | Some("LANDSCAPERIGHT") => {
                if !set_kms_orientation(crate::drivers::display_kms::DisplayOrientation::LandscapeRight, 1440, 720) {
                    serial_println!("[DISPLAY_HAL] REJECT orientation controller busy");
                    return true;
                }
                if let Some(mut sm) = crate::sensor::SENSOR_MANAGER.try_lock() {
                    sm.imu.accel = crate::sensor::Vector3D { x: 980, y: 0, z: 0 };
                }
                serial_println!("[DISPLAY_HAL] Orientation changed to LandscapeRight. KMS scanout updated to 1440x720 landscape.");
            }
            Some("LANDSCAPE_LEFT") | Some("LANDSCAPELEFT") => {
                if !set_kms_orientation(crate::drivers::display_kms::DisplayOrientation::LandscapeLeft, 1440, 720) {
                    serial_println!("[DISPLAY_HAL] REJECT orientation controller busy");
                    return true;
                }
                if let Some(mut sm) = crate::sensor::SENSOR_MANAGER.try_lock() {
                    sm.imu.accel = crate::sensor::Vector3D { x: -980, y: 0, z: 0 };
                }
                serial_println!("[DISPLAY_HAL] Orientation changed to LandscapeLeft. KMS scanout updated to 1440x720 landscape.");
            }
            Some("PORTRAIT") => {
                if !set_kms_orientation(crate::drivers::display_kms::DisplayOrientation::Portrait, 720, 1440) {
                    serial_println!("[DISPLAY_HAL] REJECT orientation controller busy");
                    return true;
                }
                if let Some(mut sm) = crate::sensor::SENSOR_MANAGER.try_lock() {
                    sm.imu.accel = crate::sensor::Vector3D { x: 0, y: 980, z: 0 };
                }
                serial_println!("[DISPLAY_HAL] Orientation changed to Portrait. KMS scanout updated to 720x1440 native scanout.");
            }
            _ => {
                serial_println!("[DISPLAY_HAL] REJECT invalid orientation option");
            }
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("RAPP_TRUST") {
        let Some(key_hex) = parts.next() else {
            serial_println!("[RAPP_HOST] REJECT trust key missing");
            return true;
        };
        let Some(publisher) = parts.next() else {
            serial_println!("[RAPP_HOST] REJECT trust publisher missing");
            return true;
        };
        if key_hex.len() != 64 || publisher.is_empty() || publisher.len() > 32 {
            serial_println!("[RAPP_HOST] REJECT trust key dimensions");
            return true;
        }
        let raw = key_hex.as_bytes();
        let mut key = [0u8; 32];
        let mut valid_hex = true;
        for i in 0..32 {
            let hi = hex_nibble(raw[i * 2]);
            let lo = hex_nibble(raw[i * 2 + 1]);
            match (hi, lo) {
                (Some(h), Some(l)) => key[i] = (h << 4) | l,
                _ => {
                    valid_hex = false;
                    break;
                }
            }
        }
        if !valid_hex {
            serial_println!("[RAPP_HOST] REJECT trust hex");
            return true;
        }
        if crate::package::revocation::is_system_publisher_identity(publisher) {
            serial_println!("[RAPP_HOST] REJECT system publisher cannot be provisioned via serial");
            return true;
        }
        let mut reg = crate::package::REVOCATION_REGISTRY.lock();
        let res = reg.trust_key(key, publisher).or_else(|_| {
            reg.grant_trust_root(key, publisher)
        });
        match res {
            Ok(()) => serial_println!("[RAPP_HOST] TRUSTED publisher={}", publisher),
            Err(err) => serial_println!("[RAPP_HOST] TRUST_FAILED {}", err),
        }
        return true;
    }

    false
}

/// Installs, lists and withdraws the device's TLS trust anchors.
///
/// WHY this is a console verb and not a syscall: an anchor is device-wide trust,
/// and an application that could add one would choose which certificate chains
/// every other application on the device accepts. The console is the channel the
/// operator of the device holds, which is the same reason `RAPP_TRUST` has no
/// syscall wrapper.
///
/// WHY the anchor can arrive two ways: one line is the friendly path, and it
/// carries at most 250 anchor bytes -- a console line holds 511 characters, the
/// verb and a space take 10 of them, and hex doubles what is left. A real root is
/// far bigger than that; a trimmed self-signed Ed25519 CA measures 274 bytes and
/// ISRG Root X1 measures 1391. So `TLS_TRUST_BEGIN` / `TLS_TRUST_CHUNK` /
/// `TLS_TRUST_COMMIT` carries the rest in the shape `RAPP_*` already uses for
/// packages: staging is bounded by `MAX_ANCHOR_DER_BYTES`, a chunk that runs past
/// the declared size is refused, and nothing is judged until commit, when the
/// whole anchor is in hand. A half-filled transfer installs nothing.
fn handle_tls_trust_command(s: &str) -> bool {
    let mut parts = s.split_ascii_whitespace();
    let Some(verb) = parts.next() else { return false; };

    if verb.eq_ignore_ascii_case("TLS_TRUST_BEGIN") {
        let Some(len) = parts.next().and_then(|v| v.parse::<usize>().ok()) else {
            serial_println!("[TLS_TRUST] REJECT anchor size missing");
            return true;
        };
        if len == 0 || len > crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES {
            serial_println!("[TLS_TRUST] REJECT anchor dimensions");
            return true;
        }
        *ANCHOR_TRANSFER.lock() = Some(AnchorTransfer {
            expected_len: len,
            der: [0u8; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES],
            received: [false; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES],
        });
        serial_println!("[TLS_TRUST] READY bytes={}", len);
        return true;
    }

    if verb.eq_ignore_ascii_case("TLS_TRUST_CHUNK") {
        let Some(offset) = parts.next().and_then(|v| v.parse::<usize>().ok()) else {
            serial_println!("[TLS_TRUST] REJECT chunk offset missing");
            return true;
        };
        let Some(hex) = parts.next() else {
            serial_println!("[TLS_TRUST] REJECT chunk data missing");
            return true;
        };
        if hex.is_empty() || hex.len() % 2 != 0 || hex.len() > ANCHOR_CHUNK_HEX_CHARS {
            serial_println!("[TLS_TRUST] REJECT chunk size");
            return true;
        }
        let mut transfer = ANCHOR_TRANSFER.lock();
        let Some(state) = transfer.as_mut() else {
            serial_println!("[TLS_TRUST] REJECT no anchor in progress");
            return true;
        };
        let chunk_len = hex.len() / 2;
        if offset.checked_add(chunk_len).map(|end| end <= state.expected_len) != Some(true) {
            serial_println!("[TLS_TRUST] REJECT chunk bounds");
            return true;
        }
        let raw = hex.as_bytes();
        for i in 0..chunk_len {
            let (Some(hi), Some(lo)) = (hex_nibble(raw[i * 2]), hex_nibble(raw[i * 2 + 1])) else {
                serial_println!("[TLS_TRUST] REJECT chunk hex");
                return true;
            };
            state.der[offset + i] = (hi << 4) | lo;
            state.received[offset + i] = true;
        }
        let received = state.received[..state.expected_len].iter().filter(|b| **b).count();
        serial_println!(
            "[TLS_TRUST] CHUNK offset={} len={} received={}/{}",
            offset,
            chunk_len,
            received,
            state.expected_len
        );
        return true;
    }

    if verb.eq_ignore_ascii_case("TLS_TRUST_COMMIT") {
        let label = parts.next().unwrap_or("");
        let mut transfer = ANCHOR_TRANSFER.lock();
        let Some(state) = transfer.take() else {
            serial_println!("[TLS_TRUST] REJECT no anchor in progress");
            return true;
        };
        if state.received[..state.expected_len].iter().any(|v| !*v) {
            serial_println!("[TLS_TRUST] REJECT incomplete anchor");
            return true;
        }
        let der = &state.der[..state.expected_len];
        let mut store = crate::crypto::tls_trust::TLS_TRUST_STORE.lock();
        return match store.install(der, label) {
            Ok(position) => {
                serial_println!(
                    "[TLS_TRUST] ANCHOR position={} bytes={} label={} total={}",
                    position,
                    der.len(),
                    label,
                    store.len()
                );
                true
            }
            Err(err) => {
                serial_println!("[TLS_TRUST] REJECT {}", err.message());
                true
            }
        };
    }

    if verb.eq_ignore_ascii_case("TLS_TRUST_LIST") {
        let store = crate::crypto::tls_trust::TLS_TRUST_STORE.lock();
        serial_println!("[TLS_TRUST] anchors={}", store.len());
        for position in 0..store.len() {
            let der = store.anchor_der(position).unwrap_or(&[]);
            let label = store.anchor_label(position).unwrap_or("");
            serial_println!("[TLS_TRUST] {} bytes={} label={}", position, der.len(), label);
        }
        return true;
    }

    if verb.eq_ignore_ascii_case("TLS_TRUST_REVOKE") {
        let Some(position) = parts.next().and_then(|v| v.parse::<usize>().ok()) else {
            serial_println!("[TLS_TRUST] REJECT revoke position missing");
            return true;
        };
        let mut store = crate::crypto::tls_trust::TLS_TRUST_STORE.lock();
        return match store.revoke(position) {
            Ok(()) => {
                serial_println!("[TLS_TRUST] REVOKED position={} remaining={}", position, store.len());
                true
            }
            Err(err) => {
                serial_println!("[TLS_TRUST] REVOKE_FAILED {}", err.message());
                true
            }
        };
    }

    if verb.eq_ignore_ascii_case("TLS_TRUST") {
        let Some(hex) = parts.next() else {
            serial_println!("[TLS_TRUST] REJECT anchor missing");
            return true;
        };
        let label = parts.next().unwrap_or("");
        // The store's own bound, which one console line can never carry this far;
        // see the handler header for why the chunk verbs exist.
        if hex.is_empty() || hex.len() % 2 != 0 || hex.len() / 2 > crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES {
            serial_println!("[TLS_TRUST] REJECT anchor dimensions");
            return true;
        }
        let raw = hex.as_bytes();
        let mut der = [0u8; crate::crypto::tls_trust::MAX_ANCHOR_DER_BYTES];
        let mut i = 0usize;
        while i < raw.len() {
            let (Some(hi), Some(lo)) = (hex_nibble(raw[i]), hex_nibble(raw[i + 1])) else {
                serial_println!("[TLS_TRUST] REJECT anchor hex");
                return true;
            };
            der[i / 2] = (hi << 4) | lo;
            i += 2;
        }
        let der = &der[..raw.len() / 2];
        let mut store = crate::crypto::tls_trust::TLS_TRUST_STORE.lock();
        return match store.install(der, label) {
            Ok(position) => {
                serial_println!(
                    "[TLS_TRUST] ANCHOR position={} bytes={} label={} total={}",
                    position,
                    der.len(),
                    label,
                    store.len()
                );
                true
            }
            Err(err) => {
                serial_println!("[TLS_TRUST] REJECT {}", err.message());
                true
            }
        };
    }

    false
}

fn handle_serial_command(cmd: &[u8]) {
    let s = if let Ok(s) = core::str::from_utf8(cmd) {
        s.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
    } else {
        return;
    };
    if s.is_empty() {
        return;
    }
    // WHY bulk transfer lines are not echoed: a `STUDIO_FS_CHUNK` carries up
    // to 512 hex characters and a file upload sends dozens of them. Echoing
    // the payload multiplied console traffic roughly twentyfold and was the
    // pressure that exposed the unbounded UART wait. The `CHUNK_OK`
    // acknowledgement still proves the chunk was received.
    let bulk = s.split_ascii_whitespace().next().is_some_and(|verb| {
        verb.eq_ignore_ascii_case("STUDIO_FS_CHUNK")
            || verb.eq_ignore_ascii_case("RAPP_CHUNK")
            || verb.eq_ignore_ascii_case("TLS_TRUST_CHUNK")
    });
    if !bulk {
        crate::serial_println!("[CMD] {}", s);
    }
    if handle_rapp_command(s) {
        return;
    }
    if handle_tls_trust_command(s) {
        return;
    }

    // The guest's filesystem channel, and the console's other guest-facing
    // commands. WHY it is asked for before the rest: the Studio sends
    // these to read and write the guest's own files, and the rest of the
    // console is the platform's own diagnostics.
    //
    // WHY the line is handed over whole: `handle_guest_fs_command` reads the
    // verb itself, and the Studio's transport writes `STUDIO_FS_LIST <path>`
    // as one line. Dropping the first token here made every one of those
    // commands start at the path, match no verb, and fall through silently.
    {
        let mut guest_parts = s.split_ascii_whitespace();
        if handle_guest_fs_command(&mut guest_parts) {
            return;
        }
    }

    let mut parts = s.split_ascii_whitespace();
    let verb = match parts.next() {
        Some(v) => v,
        None => return,
    };

    if verb.eq_ignore_ascii_case("SURFACES") {
        // WHY this verb exists: a screen that shows the wrong thing is
        // diagnosed by what the compositor holds, and the only other way to
        // see it is to add a print to the compositor's own loop.
        if let Some(kms) = crate::drivers::display_kms::DRM_KMS.try_lock() {
            for surface in kms.surfaces.iter() {
                serial_println!(
                    "[SURFACES] id={} owner={} z={} x={} y={} {}x{} visible={} ready={} has_frame={} complete={} input={}",
                    surface.id,
                    surface.owner_task,
                    surface.z_order,
                    surface.x,
                    surface.y,
                    surface.width,
                    surface.height,
                    surface.visible,
                    surface.frame_ready,
                    surface.has_frame,
                    surface.frame_complete,
                    surface.input_enabled,
                );
            }
        } else {
            serial_println!("[SURFACES] the display lock is held");
        }
        return;
    }

    if verb.eq_ignore_ascii_case("DIAG_STATE") || verb.eq_ignore_ascii_case("STATS") {
        let (tasks_tot, tasks_act) = {
            if let Some(s) = crate::task::SCHEDULER.try_lock() {
                (s.task_count(), s.active_task_count())
            } else {
                (0, 0)
            }
        };
        let surfaces = {
            if let Some(kms) = crate::drivers::display_kms::DRM_KMS.try_lock() {
                kms.surfaces.len()
            } else {
                0
            }
        };
        let (heap_used, heap_free) = crate::allocator::heap_stats();
        // Published on every flip by the compositor, unlike the sampled
        // `[KMS PERF]` line. Reporting it here is what lets a latency run tell
        // an expensive frame apart from a host that simply stopped scheduling.
        let (comp_cyc, pres_cyc, last_flip) = crate::performance::last_frame();
        let (mailboxes, endpoints, shm_pipes) = {
            let (mb, ep) = if let Some(r) = crate::ipc::IPC_ROUTER.try_lock() {
                (r.mailbox_count(), r.endpoint_count())
            } else {
                (0, 0)
            };
            let shm = if let Some(p) = crate::ipc::shm_pipe::SHM_PIPE_MANAGER.try_lock() {
                p.pipes.iter().filter(|x| x.is_some()).count()
            } else {
                0
            };
            (mb, ep, shm)
        };
        let (q_wait_last, q_wait_max, q_lines) = {
            // The wait a finished command line spent here is the part of a tap's
            // journey that no app or flip counter can show: the bytes arrived,
            // but dispatch only runs where a context chose to call
            // `service_serial_commands`.
            if let Some(state) = RX_BUF.try_lock() {
                state.wait_stats()
            } else {
                (0, 0, 0)
            }
        };
        crate::serial_println!(
            "[DIAG_STATE] tasks_total={} tasks_active={} surfaces={} mailboxes={} endpoints={} shm_pipes={} heap_used={} heap_free={} last_flip={} last_flip_compose_cycles={} last_flip_present_cycles={} input_queue_wait_last_ms={} input_queue_wait_max_ms={} input_lines={}",
            tasks_tot, tasks_act, surfaces, mailboxes, endpoints, shm_pipes, heap_used, heap_free,
            last_flip, comp_cyc, pres_cyc, q_wait_last, q_wait_max, q_lines
        );
        return;
    }

    if verb.eq_ignore_ascii_case("STUDIO_PKG_LIST") {
        crate::package::PACKAGE_REGISTRY.lock().seed();
        for pkg in crate::package::FIRST_PARTY_PACKAGES {
            crate::serial_println!(
                "[STUDIO_PKG] id={} name={} version=1.0.0 state=active process={} icon={}",
                pkg.package_id, pkg.display_name, pkg.process_name, pkg.icon
            );
        }
        let entries = crate::package::PACKAGE_REGISTRY.lock().entries().to_vec();
        for entry in entries.iter() {
            if crate::package::first_party_package(&entry.package_id).is_some() {
                continue;
            }
            let state = match entry.state {
                crate::package::PackageState::Active => "active",
                crate::package::PackageState::Staged => "staged",
                crate::package::PackageState::RolledBack => "rolledback",
            };
            crate::serial_println!(
                "[STUDIO_PKG] id={} name={} version={}.{}.{} state={} process={} icon=RAPP",
                entry.package_id, entry.package_id,
                entry.version.0, entry.version.1, entry.version.2,
                state, entry.package_id
            );
        }
        crate::serial_println!("[STUDIO_PKG] END");
        return;
    }

    if verb.eq_ignore_ascii_case("APP_LAUNCH") {
        if let Some(id) = parts.next() {
            match crate::syscall::dev_launch_app(id) {
                Ok(tid) => serial_println!("[RAPP_HOST] LAUNCHED id={} task={}", id, tid.0),
                Err(err) => serial_println!("[RAPP_HOST] LAUNCH_FAILED id={} error={:?}", id, err),
            }
        }
        return;
    }

    if verb.eq_ignore_ascii_case("WRITE_FILE") {
        let Some(path) = parts.next() else {
            crate::serial_println!("[FILE_WRITE] status=ERR_SYNTAX");
            return;
        };
        let content = if let Some(idx) = s.find(path) {
            s[idx + path.len()..].trim_start()
        } else {
            ""
        };
        if content.is_empty() {
            crate::serial_println!("[FILE_WRITE] path={} status=ERR_EMPTY_CONTENT", path);
            return;
        }

        if !path.starts_with("/data/") || path.contains("..") || path.contains("//") {
            crate::serial_println!("[FILE_WRITE] path={} status=ERR_REJECTED", path);
            return;
        }

        let mut synced = false;
        for _ in 0..100_000 {
            if let Some(mut vfs) = crate::fs::vfs::VFS.try_lock() {
                vfs.create_file(path, content.as_bytes(), false);
                match vfs.sync() {
                    Ok(_) => crate::serial_println!("[FILE_WRITE] path={} status=OK bytes={}", path, content.len()),
                    Err(_) => crate::serial_println!("[FILE_WRITE] path={} status=ERR_SYNC", path),
                }
                synced = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !synced {
            crate::serial_println!("[FILE_WRITE] path={} status=ERR_LOCKED", path);
        }
        return;
    }

    if verb.eq_ignore_ascii_case("READ_FILE") {
        if let Some(path) = parts.next() {
            if path.contains("..") || path.contains("//") {
                crate::serial_println!("[FILE_READ] path={} status=ERR_REJECTED", path);
            } else {
                let mut read_done = false;
                for _ in 0..100_000 {
                    if let Some(vfs) = crate::fs::vfs::VFS.try_lock() {
                        let mut out = [0u8; 1024];
                        match vfs.read_file_by_path(path, &mut out) {
                            Ok(n) => {
                                let content = core::str::from_utf8(&out[..n]).unwrap_or("<binary>");
                                crate::serial_println!("[FILE_READ] path={} status=OK content={}", path, content);
                            }
                            Err(_) => {
                                crate::serial_println!("[FILE_READ] path={} status=NOT_FOUND", path);
                            }
                        }
                        read_done = true;
                        break;
                    }
                    core::hint::spin_loop();
                }
                if !read_done {
                    crate::serial_println!("[FILE_READ] path={} status=ERR_LOCKED", path);
                }
            }
        }
        return;
    }

    if verb.eq_ignore_ascii_case("SYNC") || verb.eq_ignore_ascii_case("FLUSH") {
        let mut synced = false;
        for _ in 0..100_000 {
            if let Some(vfs) = crate::fs::vfs::VFS.try_lock() {
                match vfs.sync() {
                    Ok(_) => crate::serial_println!("[SYNC] status=OK"),
                    Err(_) => crate::serial_println!("[SYNC] status=ERR"),
                }
                synced = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !synced {
            crate::serial_println!("[SYNC] status=ERR_LOCKED");
        }
        return;
    }

    if verb.eq_ignore_ascii_case("FSCK") || verb.eq_ignore_ascii_case("FS_CHECK") {
        let mut executed = false;
        for _ in 0..100_000 {
            if let Some(mut ata) = crate::drivers::ATA_PRIMARY.try_lock() {
                if let Some(drive) = ata.as_mut() {
                    let total = drive.sector_count();
                    let layout = crate::storage::filesystem::FilesystemLayout::canonical();
                    let end = total.saturating_sub(layout.tail_reserved_sectors);
                    let region = end.saturating_sub(layout.base_lba);
                    if region >= crate::storage::filesystem::MIN_INTERFS_REGION_SECTORS {
                        match crate::storage::interfs::InterFs::mount_region(drive, layout.base_lba, region) {
                            Ok(mut fs) => match fs.check_integrity() {
                                Ok(report) => {
                                    crate::serial_println!(
                                        "[FSCK] status=CLEAN used_inodes={} alloc_sectors={} ref_sectors={}",
                                        report.used_inodes, report.allocated_sectors, report.referenced_sectors
                                    );
                                }
                                Err(e) => {
                                    crate::serial_println!("[FSCK] status=CORRUPT error={:?}", e);
                                }
                            },
                            Err(e) => {
                                crate::serial_println!("[FSCK] status=MOUNT_ERR error={:?}", e);
                            }
                        }
                    } else {
                        crate::serial_println!("[FSCK] status=REGION_TOO_SMALL sectors={}", region);
                    }
                } else {
                    crate::serial_println!("[FSCK] status=NO_DRIVE");
                }
                executed = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !executed {
            crate::serial_println!("[FSCK] status=DRIVE_LOCKED");
        }
        return;
    }

    if verb.eq_ignore_ascii_case("SHUTDOWN") || verb.eq_ignore_ascii_case("POWEROFF") {
        crate::serial_println!("[SYSTEM] Clean shutdown requested via COM1 serial.");
        crate::hal::traits::PowerController::shutdown(&crate::hal::platform::POWER);
    }

    if verb.eq_ignore_ascii_case("REBOOT") || verb.eq_ignore_ascii_case("RESET") {
        crate::serial_println!("[SYSTEM] System reboot requested via COM1 serial.");
        crate::hal::traits::PowerController::reset(&crate::hal::platform::POWER);
    }

    if verb.eq_ignore_ascii_case("PANIC_POLICY") {
        match parts.next() {
            Some("GET") | None => {
                let p = crate::panic::get_panic_policy();
                crate::serial_println!("[PANIC_POLICY] current={}", p.as_str());
            }
            Some("SET") => {
                if let Some(val) = parts.next() {
                    if val.eq_ignore_ascii_case("REBOOT") {
                        crate::panic::set_panic_policy(crate::panic::PanicPolicy::Reboot);
                        crate::serial_println!("[PANIC_POLICY] updated=REBOOT");
                    } else if val.eq_ignore_ascii_case("HALT") {
                        crate::panic::set_panic_policy(crate::panic::PanicPolicy::Halt);
                        crate::serial_println!("[PANIC_POLICY] updated=HALT");
                    } else {
                        crate::serial_println!("[PANIC_POLICY] err=UNKNOWN_POLICY");
                    }
                }
            }
            _ => {
                crate::serial_println!("[PANIC_POLICY] err=SYNTAX");
            }
        }
        return;
    }

    if verb.eq_ignore_ascii_case("TRIGGER_KERNEL_PANIC") {
        let reason = parts.next().unwrap_or("Manual_Diagnostic_Panic_Test");
        panic!("Controlled diagnostic fault: {}", reason);
    }

    if verb.eq_ignore_ascii_case("LOCK_KEY") {
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        kms.set_powered(true);
        let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
        let _ = display.composite_and_flip();
        crate::input::post_key_event(crate::input::KeyEvent {
            source: crate::input::InputSource::DeviceStudio,
            action: crate::input::KeyAction::Down,
            code: crate::input::KeyCode::Unknown(0x1000F),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        });
        crate::input::post_key_event(crate::input::KeyEvent {
            source: crate::input::InputSource::DeviceStudio,
            action: crate::input::KeyAction::Up,
            code: crate::input::KeyCode::Unknown(0x1000F),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        });
        crate::serial_println!("[LOCK] power key posted source=device_studio");
        return;
    }

    if verb.eq_ignore_ascii_case("POWER_KEY") || verb.eq_ignore_ascii_case("LOCK_KEY") {
        let mut kms = crate::drivers::display_kms::DRM_KMS.lock();
        let _ = kms.toggle_powered();
        kms.set_powered(true);
        let mut display = crate::drivers::KmsDisplayDriver::new(&mut kms);
        let _ = display.composite_and_flip();
        crate::input::post_key_event(crate::input::KeyEvent {
            source: crate::input::InputSource::DeviceStudio,
            action: crate::input::KeyAction::Down,
            code: crate::input::KeyCode::Unknown(0x1000F),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        });
        crate::input::post_key_event(crate::input::KeyEvent {
            source: crate::input::InputSource::DeviceStudio,
            action: crate::input::KeyAction::Up,
            code: crate::input::KeyCode::Unknown(0x1000F),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
        });
        crate::serial_println!("[POWER] display_on=true source=device_studio");
        return;
    }

    if verb.eq_ignore_ascii_case("VOLUME") {
        let action = parts.next().unwrap_or("GET");
        let mut audio = crate::audio::AUDIO_MANAGER.lock();
        // WHY: `AudioManager::volume_percent` is an `Option<u8>` that boots as
        // `None`. It used to be a `u8` seeded with a hardcoded `100`, so a bare
        // `VOLUME GET` reported 100% on a machine that had never measured a
        // level. `Option<u8>` has no `Display`, and substituting a placeholder
        // number would put the fabrication straight back into this log line, so
        // each branch emits the real state instead.
        //
        // WHY `UNBOUND` as the token: `emulator/server.py` scrapes this line with
        // `re.search(r'volume_percent=(\d+)', line)`. A non-digit token simply
        // does not match, so the host correctly falls into its existing
        // "no reading" branch (`status: 'UNCONFIRMED'`) rather than being handed
        // an invented percentage.
        fn report_volume(audio: &crate::audio::AudioManager) {
            match audio.volume_percent {
                Some(requested) => crate::serial_println!(
                    "[AUDIO] volume_percent={} state=requested applied_to_hardware=no source=serial_console",
                    requested
                ),
                None => crate::serial_println!(
                    "[AUDIO] volume_percent=UNBOUND state=unbound applied_to_hardware=no source=serial_console"
                ),
            }
        }
        if action.eq_ignore_ascii_case("UP") {
            // WHY: stepping needs a known level to step *from*. With `None`
            // stored there is nothing to move, and the old code silently derived
            // a base from the seeded 100 instead of saying so.
            audio.step_volume(5);
            report_volume(&audio);
        } else if action.eq_ignore_ascii_case("DOWN") {
            audio.step_volume(-5);
            report_volume(&audio);
        } else if action.eq_ignore_ascii_case("MUTE") {
            audio.set_volume(0);
            report_volume(&audio);
        } else if let Ok(pct) = action.parse::<u8>() {
            audio.set_volume(pct.min(100));
            report_volume(&audio);
        } else {
            report_volume(&audio);
        }
        return;
    }

    if verb.eq_ignore_ascii_case("SENSOR_INJECT") {
        let x = parts.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let y = parts.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(980);
        let z = parts.next().and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        let mut sm = crate::sensor::SENSOR_MANAGER.lock();
        sm.imu.update_accel(crate::sensor::Vector3D::new(x, y, z));
        if let (Some(gx), Some(gy), Some(gz)) = (
            parts.next().and_then(|v| v.parse::<i32>().ok()),
            parts.next().and_then(|v| v.parse::<i32>().ok()),
            parts.next().and_then(|v| v.parse::<i32>().ok()),
        ) {
            sm.imu.update_gyro(crate::sensor::Vector3D::new(gx, gy, gz));
        }
        let orient = sm.imu.determine_orientation();
        crate::serial_println!(
            "[SENSOR_INJECT] accel=({}, {}, {}) orientation={:?}",
            x, y, z, orient
        );
        return;
    }

    if verb.eq_ignore_ascii_case("BATTERY_SET") {
        let pct = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(80).min(100);
        let charging_str = parts.next().unwrap_or("DISCHARGING");
        let charging_status = match charging_str.to_ascii_uppercase().as_str() {
            "AC" | "CHARGING" | "1" | "TRUE" | "CHARGINGAC" | "USB" => crate::resource::battery::ChargingStatus::ChargingAC,
            "WIRELESS" | "CHARGINGWIRELESS" => crate::resource::battery::ChargingStatus::ChargingWireless,
            "FULL" => crate::resource::battery::ChargingStatus::Full,
            _ => crate::resource::battery::ChargingStatus::Discharging,
        };
        let design_capacity = 5000u32;
        let capacity_mah = ((design_capacity as u64 * pct as u64) / 100) as u32;
        let voltage_mv = 3500 + ((700 * pct as u32) / 100);
        let temperature_c = 28i8;
        let mut gauge = crate::resource::BATTERY_GAUGE.lock();
        gauge.set_telemetry(voltage_mv, capacity_mah, temperature_c, charging_status);
        crate::serial_println!(
            "[BATTERY_SET] soc_percent={} charging_status={:?} voltage_mv={} capacity_mah={}",
            pct, charging_status, voltage_mv, capacity_mah
        );
        return;
    }

    if verb.eq_ignore_ascii_case("FINGERPRINT_INJECT") {
        let result_str = parts.next().unwrap_or("PASS");
        if result_str.eq_ignore_ascii_case("PASS") {
            let mut um = crate::user::USER_MANAGER.lock();
            let _ = um.unlock_biometric(crate::user::UserId::OWNER);
            drop(um);
            crate::input::post_key_event(crate::input::KeyEvent {
                source: crate::input::InputSource::DeviceStudio,
                action: crate::input::KeyAction::Down,
                code: crate::input::KeyCode::Unknown(0x1000F),
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            });
            crate::input::post_key_event(crate::input::KeyEvent {
                source: crate::input::InputSource::DeviceStudio,
                action: crate::input::KeyAction::Up,
                code: crate::input::KeyCode::Unknown(0x1000F),
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            });
            crate::serial_println!("[FINGERPRINT_INJECT] PASS: biometric match verified, UID=1000 unlocked");
        } else {
            crate::serial_println!("[FINGERPRINT_INJECT] FAIL: biometric match rejected");
        }
        return;
    }

    if verb.eq_ignore_ascii_case("KILLSWITCH_TOGGLE") {
        let subsystem = parts.next().unwrap_or("ALL");
        let state_str = parts.next().unwrap_or("ON");
        let engaged = state_str.eq_ignore_ascii_case("ON") || state_str.eq_ignore_ascii_case("1") || state_str.eq_ignore_ascii_case("TRUE") || state_str.eq_ignore_ascii_case("CUT");
        let mut ks = HARDWARE_KILLSWITCHES.lock();
        match subsystem.to_ascii_uppercase().as_str() {
            "CAMERA" | "CAM" => ks.camera_cut = engaged,
            "MIC" | "MICROPHONE" | "AUDIO" => ks.mic_cut = engaged,
            "CELLULAR" | "CELL" | "MODEM" | "RADIO" => ks.cellular_cut = engaged,
            "GPS" | "LOCATION" => ks.gps_cut = engaged,
            "ALL" => {
                ks.camera_cut = engaged;
                ks.mic_cut = engaged;
                ks.cellular_cut = engaged;
                ks.gps_cut = engaged;
            }
            _ => {
                crate::serial_println!("[KILLSWITCH] REJECT unknown subsystem: {}", subsystem);
                return;
            }
        }
        crate::serial_println!(
            "[KILLSWITCH] subsystem={} engaged={} (camera_cut={} mic_cut={} cellular_cut={} gps_cut={})",
            subsystem, engaged, ks.camera_cut, ks.mic_cut, ks.cellular_cut, ks.gps_cut
        );
        return;
    }

    if verb.eq_ignore_ascii_case("KILLSWITCH_STATUS") || verb.eq_ignore_ascii_case("KILLSWITCH_GET") {
        let ks = HARDWARE_KILLSWITCHES.lock();
        crate::serial_println!(
            "[KILLSWITCH_STATUS] camera_cut={} mic_cut={} cellular_cut={} gps_cut={}",
            ks.camera_cut, ks.mic_cut, ks.cellular_cut, ks.gps_cut
        );
        return;
    }

    // Legacy convenience touch verbs. These accept the same logical portrait
    // coordinates as INPUT_TOUCH and are bounds checked identically: an
    // unchecked point here would be injected straight into the gesture router.
    const TOUCH_MAX_X: u32 = 720;
    const TOUCH_MAX_Y: u32 = 1440;
    fn touch_point(x: u32, y: u32) -> Result<(u32, u32), ()> {
        if x < TOUCH_MAX_X && y < TOUCH_MAX_Y { Ok((x, y)) } else { Err(()) }
    }

    if verb.eq_ignore_ascii_case("TAP") || verb.eq_ignore_ascii_case("TOUCH") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<u32>(), y_str.parse::<u32>()) {
                match touch_point(x, y) {
                    Ok((x, y)) => {
                        crate::serial_println!("[COM1 INPUT] Hardware touch tap received at ({}, {}).", x, y);
                        crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Down);
                        crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up);
                    }
                    Err(()) => crate::serial_println!("[COM1 INPUT] REJECT tap out of bounds ({}, {})", x, y),
                }
            }
        }
    } else if verb.eq_ignore_ascii_case("DOWN") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<u32>(), y_str.parse::<u32>()) {
                match touch_point(x, y) {
                    Ok((x, y)) => { crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Down); }
                    Err(()) => crate::serial_println!("[COM1 INPUT] REJECT down out of bounds ({}, {})", x, y),
                }
            }
        }
    } else if verb.eq_ignore_ascii_case("MOVE") {
        if let (Some(x_str), Some(y_str)) = (parts.next(), parts.next()) {
            if let (Ok(x), Ok(y)) = (x_str.parse::<u32>(), y_str.parse::<u32>()) {
                match touch_point(x, y) {
                    Ok((x, y)) => { crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Move); }
                    Err(()) => crate::serial_println!("[COM1 INPUT] REJECT move out of bounds ({}, {})", x, y),
                }
            }
        }
    } else if verb.eq_ignore_ascii_case("UP") {
        let (x, y) = match (parts.next(), parts.next()) {
            (Some(xs), Some(ys)) => (xs.parse::<u32>().unwrap_or(u32::MAX), ys.parse::<u32>().unwrap_or(u32::MAX)),
            _ => (u32::MAX, u32::MAX),
        };
        match touch_point(x, y) {
            Ok((x, y)) => { crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up); }
            Err(()) => crate::serial_println!("[COM1 INPUT] REJECT up out of bounds ({}, {})", x, y),
        }
    } else if verb.eq_ignore_ascii_case("SWIPE") || verb.eq_ignore_ascii_case("DRAG") {
        if let (Some(x1_s), Some(y1_s), Some(x2_s), Some(y2_s)) = (parts.next(), parts.next(), parts.next(), parts.next()) {
            if let (Ok(x1), Ok(y1), Ok(x2), Ok(y2)) = (x1_s.parse::<u32>(), y1_s.parse::<u32>(), x2_s.parse::<u32>(), y2_s.parse::<u32>()) {
                // Reject the whole gesture if either endpoint is outside the
                // logical surface, rather than clamping into a bogus path.
                if let Err(()) = touch_point(x1, y1).and_then(|_| touch_point(x2, y2)) {
                    crate::serial_println!(
                        "[COM1 INPUT] REJECT swipe out of bounds ({}, {}) -> ({}, {})",
                        x1, y1, x2, y2
                    );
                    return;
                }
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
    } else if verb.eq_ignore_ascii_case("PIN") || verb.eq_ignore_ascii_case("UNLOCK") {
        if let Some(pin_str) = parts.next() {
            crate::serial_println!("[COM1 INPUT] Hardware PIN entry received: {} digits.", pin_str.len());
            for c in pin_str.chars() {
                // WHY these coordinates: they are the centres of the lock screen's
                // own key cells (`hit_test_pin_pad` in apps/launcher/src/main.rs:
                // left 130, top 520, cell 140x104, x-step 164, y-step 128; the '0'
                // row sits at 904..1008 and the CLR/backspace band at 1036..1126).
                // The previous table was one pad revision behind, so every digit
                // landed on a different key and this helper scrambled entries
                // instead of driving them.
                let coords = match c {
                    '1' => Some((200, 572)),
                    '2' => Some((364, 572)),
                    '3' => Some((528, 572)),
                    '4' => Some((200, 700)),
                    '5' => Some((364, 700)),
                    '6' => Some((528, 700)),
                    '7' => Some((200, 828)),
                    '8' => Some((364, 828)),
                    '9' => Some((528, 828)),
                    '0' => Some((364, 956)),
                    'c' | 'C' => Some((280, 1081)),
                    'x' | 'X' => Some((540, 1081)),
                    _ => None,
                };
                if let Some((x, y)) = coords {
                    crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Down);
                    crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up);
                }
            }
        }
    } else if verb.eq_ignore_ascii_case("NAV") {
        if let Some(target) = parts.next() {
            if target.eq_ignore_ascii_case("BACK") {
                crate::serial_println!("[COM1 INPUT] Hardware NAV BACK event received.");
                crate::drivers::input::dispatch_to_focused_window(crate::drivers::input::InputEvent {
                    kind: crate::drivers::input::InputEventKind::Back,
                    x: 120,
                    y: 1416,
                    ..crate::drivers::input::InputEvent::EMPTY
                });
                crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
            } else if target.eq_ignore_ascii_case("HOME") {
                crate::serial_println!("[COM1 INPUT] Hardware NAV HOME event received.");
                crate::drivers::input::dispatch_to_focused_window(crate::drivers::input::InputEvent {
                    kind: crate::drivers::input::InputEventKind::Home,
                    x: 360,
                    y: 1416,
                    ..crate::drivers::input::InputEvent::EMPTY
                });
                crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
            } else if target.eq_ignore_ascii_case("RECENTS") {
                crate::serial_println!("[COM1 INPUT] Hardware NAV RECENTS event received.");
                crate::drivers::input::dispatch_to_focused_window(crate::drivers::input::InputEvent {
                    kind: crate::drivers::input::InputEventKind::Recents,
                    x: 600,
                    y: 1416,
                    ..crate::drivers::input::InputEvent::EMPTY
                });
                crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
            }
        }
    } else if verb.eq_ignore_ascii_case("POWER_KEY") || verb.eq_ignore_ascii_case("LOCK_KEY") {
        crate::serial_println!("[COM1 INPUT] Hardware POWER/LOCK key event received.");
        let key_ev = crate::drivers::input::InputEvent {
            kind: crate::drivers::input::InputEventKind::Key,
            x: 0x1000F, // Power keycode
            y: 1, // Pressed
            ..crate::drivers::input::InputEvent::EMPTY
        };
        crate::drivers::input::dispatch_to_focused_window(key_ev);
        crate::event::broadcast_event(crate::event::EVENT_TOUCH_INPUT);
    } else if verb.eq_ignore_ascii_case("INPUT_KEY") {
        if let Some(cp_s) = parts.next() {
            if let Ok(cp) = cp_s.parse::<u32>() {
                if let Some(c) = char::from_u32(cp) {
                    crate::input::post_char_event(c);
                }
            }
        }
    } else if s.len() == 1 {
        let c = s.chars().next().unwrap();
        crate::input::post_char_event(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_print_try_lock_does_not_deadlock_when_locked() {
        let guard = SERIAL1.lock();
        // With SERIAL1 held, _print_internal must attempt try_lock, exhaust budget,
        // and return false without deadlocking or panicking.
        let acquired = _print_internal(format_args!("panic path test: {}\n", "deadlock avoided"));
        assert!(!acquired, "Expected _print_internal to return false when SERIAL1 is held");

        // Calling _print directly while locked must also return safely without hanging
        _print(format_args!("direct _print call under lock: {}\n", 1));

        drop(guard);

        // When unlocked, _print_internal acquires the lock and succeeds without panic
        let acquired_unlocked = _print_internal(format_args!("normal print test: {}\n", 42));
        assert!(acquired_unlocked, "Expected _print_internal to succeed when SERIAL1 is unlocked");

        // Macro invocation in unlocked state must succeed without panic
        serial_println!("Macro invocation test {}", 100);
    }

    #[test]
    fn test_serial_commands_extended_controls() {
        // Test SENSOR_INJECT
        handle_serial_command(b"SENSOR_INJECT 123 456 789");
        let sm = crate::sensor::SENSOR_MANAGER.lock();
        assert_eq!(sm.imu.accel.x, 123);
        assert_eq!(sm.imu.accel.y, 456);
        assert_eq!(sm.imu.accel.z, 789);
        drop(sm);

        // Test BATTERY_SET
        handle_serial_command(b"BATTERY_SET 60 AC");
        let bg = crate::resource::BATTERY_GAUGE.lock();
        assert_eq!(bg.soc_percent, 60);
        assert_eq!(bg.charging_status, crate::resource::battery::ChargingStatus::ChargingAC);
        drop(bg);

        // Test FINGERPRINT_INJECT
        handle_serial_command(b"FINGERPRINT_INJECT PASS");
        let um = crate::user::USER_MANAGER.lock();
        if let Ok(state) = um.get_lock_state(crate::user::UserId::OWNER) {
            assert_eq!(state, crate::user::UserLockState::Unlocked);
        }
        drop(um);

        // Test KILLSWITCH_TOGGLE
        handle_serial_command(b"KILLSWITCH_TOGGLE CAMERA ON");
        assert!(HARDWARE_KILLSWITCHES.lock().camera_cut);
        handle_serial_command(b"KILLSWITCH_TOGGLE CAMERA OFF");
        assert!(!HARDWARE_KILLSWITCHES.lock().camera_cut);
    }

    #[test]
    fn a_trust_anchor_arrives_over_several_console_lines() {
        use alloc::format;

        crate::crypto::tls_trust::TLS_TRUST_STORE.lock().clear();

        // One complete DER sequence carrying the Ed25519 algorithm identifier --
        // the shape the store gates on -- sent as two chunks so the framing, not
        // the certificate, is what this case exercises. The length header is the
        // short form because a `0x81` prefix below 0x80 is BER, which the store
        // refuses.
        let mut der: Vec<u8> = Vec::new();
        der.extend_from_slice(&[0x30, 38, 0x06, 0x03, 0x2b, 0x65, 0x70]);
        der.resize(40, 0x41);
        let hex: String = der.iter().map(|b| format!("{:02x}", b)).collect();

        handle_serial_command(b"TLS_TRUST_BEGIN 40");
        handle_serial_command(format!("TLS_TRUST_CHUNK 0 {}", &hex[..40]).as_bytes());
        handle_serial_command(format!("TLS_TRUST_CHUNK 20 {}", &hex[40..]).as_bytes());
        handle_serial_command(b"TLS_TRUST_COMMIT probe-ca");

        let store = crate::crypto::tls_trust::TLS_TRUST_STORE.lock();
        assert_eq!(store.len(), 1, "a chunked anchor must occupy exactly one slot");
        assert_eq!(store.anchor_der(0), Some(der.as_slice()));
        assert_eq!(store.anchor_label(0), Some("probe-ca"));
        drop(store);

        // A chunk that runs past the declared size is refused, and the anchor it
        // belonged to never reaches the store.
        crate::crypto::tls_trust::TLS_TRUST_STORE.lock().clear();
        handle_serial_command(b"TLS_TRUST_BEGIN 8");
        handle_serial_command(b"TLS_TRUST_CHUNK 0 ffffffffffffffffffffffffffff");
        handle_serial_command(b"TLS_TRUST_COMMIT overshoot");
        assert!(
            crate::crypto::tls_trust::TLS_TRUST_STORE.lock().is_empty(),
            "a chunk outside the declared anchor was accepted"
        );

        // A byte that never arrived is caught at commit rather than left as the
        // zero the staging buffer was filled with.
        handle_serial_command(b"TLS_TRUST_BEGIN 40");
        handle_serial_command(format!("TLS_TRUST_CHUNK 0 {}", &hex[..20]).as_bytes());
        handle_serial_command(b"TLS_TRUST_COMMIT half");
        assert!(
            crate::crypto::tls_trust::TLS_TRUST_STORE.lock().is_empty(),
            "an incomplete anchor was installed"
        );

        // Committing with nothing staged answers for itself.
        handle_serial_command(b"TLS_TRUST_COMMIT nothing");
        assert!(crate::crypto::tls_trust::TLS_TRUST_STORE.lock().is_empty());
    }
}

