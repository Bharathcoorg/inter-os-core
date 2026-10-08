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
use crate::storage::block::BlockDevice;

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

    // WHY: `without_interrupts` emits `cli`/`sti`, which are privileged. A host
    // unit-test binary runs in ring 3, so taking that path raised
    // STATUS_PRIVILEGED_INSTRUCTION and killed the test process before any
    // assertion ran. The masking exists solely to stop an interrupt handler
    // deadlocking against this lock; a test build has no COM1 and no timer
    // interrupt, so there is nothing to mask against.
    #[cfg(not(test))]
    {
        use x86_64::instructions::interrupts;

        // Print without interrupts to avoid deadlock if an interrupt handler also logs.
        interrupts::without_interrupts(|| {
            if let Some(ref mut serial) = *SERIAL1.lock() {
                serial.write_fmt(args).expect("Printing to serial port failed");
            }
        });
    }

    // WHY: no COM1 exists in a host test process. Output is deliberately dropped
    // instead of executing privileged port I/O, so `cfg(test)` builds can assert
    // on behaviour without pretending a console is attached.
    #[cfg(test)]
    {
        let _ = args;
    }
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

/// Number of completed command lines that can be queued between the COM1
/// interrupt and the main loop's dispatch pass.
///
/// WHY a queue rather than a single buffer: the interrupt handler must not
/// dispatch, so completed lines have to be held somewhere. With a single buffer
/// a second line arriving before the main loop runs would be appended to the
/// first and the two would execute as one garbled command. Four slots is ample
/// for a human or a test harness, and the overflow is reported rather than
/// silently dropping the oldest.
const RX_QUEUE_SLOTS: usize = 16;

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
}

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
        out[..len].copy_from_slice(&self.queue[self.qhead][..len]);
        self.qhead = (self.qhead + 1) % RX_QUEUE_SLOTS;
        self.qcount -= 1;
        len
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
/// The symptom was intermittent and misleading: a `TAP` sent over the serial
/// console would sometimes appear to do nothing at all, which looked like a UI
/// defect and sent the investigation into the launcher and compositor.
///
/// The handler now only accumulates bytes, which is bounded and cannot block on
/// anything but the short `RX_BUF` lock. Dispatch happens in
/// `service_serial_commands`, called from the main loop.
pub fn poll_serial_rx() {
    let mut lsr = Port::<u8>::new(0x3FD);
    let mut rbr = Port::<u8>::new(0x3F8);

    let mut count = 0;
    while count < 64 {
        let status = unsafe { lsr.read() };
        if (status & 1) == 0 {
            break;
        }
        let byte = unsafe { rbr.read() };
        count += 1;

        // WHY an overflow marker rather than a silent drop: a line longer than the
        // buffer must not be dispatched as if it were complete, and must not be
        // discarded without trace either. The marker makes the truncation visible
        // to the command handler.
        let mut state = RX_BUF.lock();
        if byte == b'\n' || byte == b'\r' {
            // A line is complete. WHY it is queued rather than executed here: the
            // handler must not launch apps or print. WHY it is queued rather than
            // flushed by the next newline: two commands can arrive before the main
            // loop runs, and concatenating them would execute one as a garbled
            // prefix of the other.
            if state.overflowed {
                // Discard the truncated line rather than executing a prefix of it.
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
}

/// Dispatches any complete serial command lines. Must be called from a normal
/// (non-interrupt) context.
pub fn service_serial_commands() {
    loop {
        // Copy the line out and release the lock before dispatching:
        // handle_serial_command prints, and printing while holding RX_BUF would
        // block the interrupt handler that fills it.
        let mut local = [0u8; 512];
        let (len, dropped) = {
            let mut state = RX_BUF.lock();
            let dropped = state.dropped;
            state.dropped = 0;
            (state.pop_line(&mut local), dropped)
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
        while start < end && (local[start] as char).is_ascii_whitespace() {
            start += 1;
        }
        while end > start && (local[end - 1] as char).is_ascii_whitespace() {
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
        let Some(x) = parts.next().and_then(|v| v.parse::<u32>().ok()) else { return true; };
        let Some(y) = parts.next().and_then(|v| v.parse::<u32>().ok()) else { return true; };
        let Some(slot) = parts.next().and_then(|v| v.parse::<u8>().ok()) else { return true; };
        // INPUT_TOUCH carries logical portrait coordinates, the same
        // CoordinateSpace the compositor, window manager and apps use. Mapping
        // the guest's letterboxed scanout back into UI space here would duplicate
        // the presentation geometry in a second place with different constants,
        // which previously rejected every gesture above the landscape midpoint.
        const LOGICAL_WIDTH: u32 = 720;
        const LOGICAL_HEIGHT: u32 = 1440;
        if x >= LOGICAL_WIDTH || y >= LOGICAL_HEIGHT || slot >= crate::input::MAX_TOUCH_SLOTS as u8 {
            serial_println!(
                "[COM1 INPUT] REJECT touch bounds x={} y={} (logical {}x{})",
                x, y, LOGICAL_WIDTH, LOGICAL_HEIGHT
            );
            return true;
        }
        crate::input::post_touch_event(slot, x, y, if phase == crate::input::TouchPhase::Up { 0 } else { 512 }, phase);
        serial_println!("[COM1 INPUT] TOUCH phase={:?} slot={} x={} y={}", phase, slot, x, y);
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

    if verb.eq_ignore_ascii_case("ORIENTATION") {
        let opt = parts.next().map(|v| v.to_ascii_uppercase());
        let set_kms_orientation = |orient: crate::drivers::display_kms::DisplayOrientation| {
            for _ in 0..100 {
                if let Some(mut kms) = crate::drivers::display_kms::DRM_KMS.try_lock() {
                    kms.set_orientation(orient);
                    return true;
                }
                core::hint::spin_loop();
            }
            false
        };
        match opt.as_deref() {
            Some("LANDSCAPE") | Some("LANDSCAPE_RIGHT") | Some("LANDSCAPERIGHT") => {
                set_kms_orientation(crate::drivers::display_kms::DisplayOrientation::LandscapeRight);
                if let Some(mut sm) = crate::sensor::SENSOR_MANAGER.try_lock() {
                    sm.imu.accel = crate::sensor::Vector3D { x: 980, y: 0, z: 0 };
                }
                serial_println!("[DISPLAY_HAL] Orientation changed to LandscapeRight. KMS scanout updated to 1280x720 full landscape.");
            }
            Some("LANDSCAPE_LEFT") | Some("LANDSCAPELEFT") => {
                set_kms_orientation(crate::drivers::display_kms::DisplayOrientation::LandscapeLeft);
                if let Some(mut sm) = crate::sensor::SENSOR_MANAGER.try_lock() {
                    sm.imu.accel = crate::sensor::Vector3D { x: -980, y: 0, z: 0 };
                }
                serial_println!("[DISPLAY_HAL] Orientation changed to LandscapeLeft. KMS scanout updated to 1280x720 full landscape.");
            }
            Some("PORTRAIT") => {
                set_kms_orientation(crate::drivers::display_kms::DisplayOrientation::Portrait);
                if let Some(mut sm) = crate::sensor::SENSOR_MANAGER.try_lock() {
                    sm.imu.accel = crate::sensor::Vector3D { x: 0, y: 980, z: 0 };
                }
                serial_println!("[DISPLAY_HAL] Orientation changed to Portrait. KMS scanout updated to 720x1440 centered scanout.");
            }
            _ => {
                serial_println!("[DISPLAY_HAL] REJECT invalid orientation option");
            }
        }
        return true;
    }

    // RAPP_TRUST IS DELIBERATELY NOT IMPLEMENTED.
    //
    // This COM1 line is an unauthenticated channel: anything that can write
    // bytes to the UART (the emulator host bridge, a physical debug header, a
    // malicious VM monitor) can emit any command. The removed handler called
    // `RevocationRegistry::trust_key`, which inserts a publisher key into the
    // trust store that `parse_rapp` consults. That made "type 64 hex chars on a
    // serial line" equivalent to "choose the publisher whose signature this
    // device will accept", and a self-signed RAPP against that publisher key is
    // ring3 code execution. A trust root must be established by an
    // authenticated provisioning path (secure boot / MDM / fused secret), never
    // by a transport that carries no authentication.
    //
    // The command is still recognised so the host tooling receives an explicit
    // refusal rather than silently treating the line as unrecognised input.
    if verb.eq_ignore_ascii_case("RAPP_TRUST") {
        serial_println!(
            "[RAPP_HOST] REJECT RAPP_TRUST is not a supported command: an unauthenticated \
             serial line must not be able to install a publisher root of trust."
        );
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
    crate::serial_println!("[CMD] {}", s);
    if handle_rapp_command(s) {
        return;
    }

    let mut parts = s.split_ascii_whitespace();
    let verb = match parts.next() {
        Some(v) => v,
        None => return,
    };

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
        crate::serial_println!(
            "[DIAG_STATE] tasks_total={} tasks_active={} surfaces={} mailboxes={} endpoints={} shm_pipes={} heap_used={} heap_free={}",
            tasks_tot, tasks_act, surfaces, mailboxes, endpoints, shm_pipes, heap_used, heap_free
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
        for _ in 0..10_000 {
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
            } else if let Some(vfs) = crate::fs::vfs::VFS.try_lock() {
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
            } else {
                crate::serial_println!("[FILE_READ] path={} status=ERR_LOCKED", path);
            }
        }
        return;
    }

    if verb.eq_ignore_ascii_case("SYNC") || verb.eq_ignore_ascii_case("FLUSH") {
        let mut synced = false;
        for _ in 0..10_000 {
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
        for _ in 0..10_000 {
            if let Some(mut ata) = crate::drivers::ATA_PRIMARY.try_lock() {
                if let Some(drive) = ata.as_mut() {
                    let total = drive.sector_count();
                    let layout = crate::storage::filesystem::FilesystemLayout::canonical();
                    let end = total.saturating_sub(layout.tail_reserved_sectors);
                    let region = end.saturating_sub(layout.base_lba).min(crate::storage::filesystem::MAX_INTERFS_REGION_SECTORS);
                    if region > 42 {
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

    if verb.eq_ignore_ascii_case("VOLUME") {
        let action = parts.next().unwrap_or("GET");
        let mut audio = crate::audio::AUDIO_MANAGER.lock();
        if action.eq_ignore_ascii_case("UP") {
            let cur = audio.volume_percent;
            audio.set_volume(cur.saturating_add(5).min(100));
            crate::serial_println!("[AUDIO] volume_percent={} state=unmuted", audio.volume_percent);
        } else if action.eq_ignore_ascii_case("DOWN") {
            let cur = audio.volume_percent;
            audio.set_volume(cur.saturating_sub(5));
            crate::serial_println!("[AUDIO] volume_percent={} state=unmuted", audio.volume_percent);
        } else if action.eq_ignore_ascii_case("MUTE") {
            audio.set_volume(0);
            crate::serial_println!("[AUDIO] volume_percent=0 state=muted");
        } else if let Ok(pct) = action.parse::<u8>() {
            audio.set_volume(pct.min(100));
            crate::serial_println!("[AUDIO] volume_percent={} state=unmuted", audio.volume_percent);
        } else {
            crate::serial_println!("[AUDIO] volume_percent={} state=unmuted", audio.volume_percent);
        }
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
                let coords = match c {
                    '1' => Some((160, 480)),
                    '2' => Some((360, 480)),
                    '3' => Some((560, 480)),
                    '4' => Some((160, 620)),
                    '5' => Some((360, 620)),
                    '6' => Some((560, 620)),
                    '7' => Some((160, 760)),
                    '8' => Some((360, 760)),
                    '9' => Some((560, 760)),
                    '0' => Some((360, 900)),
                    'c' | 'C' => Some((160, 900)),
                    'x' | 'X' => Some((560, 900)),
                    _ => None,
                };
                if let Some((x, y)) = coords {
                    crate::input::post_touch_event(0, x, y, 1000, crate::input::TouchPhase::Down);
                    crate::input::post_touch_event(0, x, y, 0, crate::input::TouchPhase::Up);
                }
            }
        }
    } else if s.len() == 1 {
        let c = s.chars().next().unwrap();
        crate::input::post_char_event(c);
    }
}
