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

struct RxState {
    buf: [u8; 512],
    len: usize,
}

impl RxState {
    const fn new() -> Self {
        Self {
            buf: [0u8; 512],
            len: 0,
        }
    }
}

/// Polls the 16550 COM1 UART receiver and dispatches input commands.
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

        let mut state = RX_BUF.lock();
        if byte == b'\n' || byte == b'\r' {
            if state.len > 0 {
                let len = state.len;
                state.len = 0;
                let slice = &state.buf[..len];
                handle_serial_command(slice);
            }
        } else if state.len < 511 {
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

    if verb.eq_ignore_ascii_case("WRITE_FILE") {
        let path = parts.next();
        let content = path.and_then(|p| {
            let prefix_len = verb.len();
            let after_verb = s[prefix_len..].trim_start();
            let after_path = after_verb[p.len()..].trim_start();
            if after_path.is_empty() { None } else { Some(after_path) }
        });
        if let (Some(path), Some(content)) = (path, content) {
            if !path.starts_with("/data/") || path.contains("..") || path.contains("//") {
                crate::serial_println!("[FILE_WRITE] path={} status=ERR_REJECTED", path);
            } else if let Some(mut vfs) = crate::fs::vfs::VFS.try_lock() {
                vfs.create_file(path, content.as_bytes(), false);
                match vfs.sync() {
                    Ok(_) => crate::serial_println!("[FILE_WRITE] path={} status=OK bytes={}", path, content.len()),
                    Err(_) => crate::serial_println!("[FILE_WRITE] path={} status=ERR_SYNC", path),
                }
            } else {
                crate::serial_println!("[FILE_WRITE] path={} status=ERR_LOCKED", path);
            }
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
        if let Some(vfs) = crate::fs::vfs::VFS.try_lock() {
            match vfs.sync() {
                Ok(_) => crate::serial_println!("[SYNC] status=OK"),
                Err(_) => crate::serial_println!("[SYNC] status=ERR"),
            }
        }
        return;
    }

    if verb.eq_ignore_ascii_case("FSCK") || verb.eq_ignore_ascii_case("FS_CHECK") {
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
        } else {
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
    } else if s.len() == 1 {
        let c = s.chars().next().unwrap();
        crate::input::post_char_event(c);
    }
}
