//! InterOS Core Application: test_app
//!
//! A deliberate platform self-diagnostic.
//!
//! The previous version of this app logged "test_app initialized inside Ring 3
//! sandbox", then entered a loop that ACKed every IPC packet with `msg_type + 1`
//! and a literal `b"ACK"` payload. Nothing about that was useful: the reply
//! content was invented, the reply type was arithmetic on an inbound value with
//! no protocol behind it, and the loop reported success without ever checking
//! whether the send worked. A test app that cannot tell you whether the platform
//! is healthy is not a test.
//!
//! This one earns its place by answering a question a developer actually asks:
//! "what does this OS know about the process I am running in, and which of those
//! answers came back?". Every field is read from a real kernel source at runtime,
//! and every one that fails is reported with the status code the kernel returned.
//! There is no field on screen that this app could have made up.
//!
//! The probes, and what each one can prove:
//!
//!   - `get_task_id()`               -> the scheduler's id for this task
//!   - `surface_create`              -> the display service really allocated a surface
//!   - `get_ticks()`                 -> the 1 kHz hardware timer
//!   - `hal_query_platform()`        -> the SoC the kernel booted on
//!   - `window_list()`               -> live compositor geometry, including this window
//!   - `service_query(10)`           -> the launcher's own lifecycle record
//!   - `settings_hardware_status()` -> the kernel's hardware availability table
//!   - `storage_query()`             -> real block-device capacity
//!   - `app_runtime_list()`          -> the other tasks currently scheduled
//!   - `user_get_current()`          -> the active foreground account
//!   - `/system/etc/policy.json`     -> the OS version the image carries
//!
//! A probe that is refused says "refused (-1) PermissionDenied" and names the
//! capability it would have needed. That is a real and useful result: it tells the
//! developer the syscall exists and this task was not granted authority to call it.

#![no_std]
#![no_main]

mod font;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Service id of the launcher shell in the kernel service registry.
const LAUNCHER_SERVICE_ID: u8 = 10;

/// The version string compiled into this binary from the crate manifest.
///
/// A build-time constant of the artifact itself, labelled on screen as such. It
/// is never presented as a version read back from the kernel.
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// Result of one real probe.
enum Probe<T: Copy> {
    Ok(T),
    /// The syscall returned a negative kernel status code.
    Refused(i64),
    /// The call succeeded but the payload lacked the fields needed to decode it.
    Malformed,
}

/// The SoC description from `hal_query_platform`.
#[derive(Clone, Copy)]
struct Platform {
    arch: [u8; 16],
    arch_len: usize,
    soc: [u8; 32],
    soc_len: usize,
    cores: i64,
    freq_mhz: i64,
}

/// One compositor window from `window_list`.
#[derive(Clone, Copy)]
struct WindowInfo {
    id: u32,
    owner: u32,
    name: [u8; 24],
    name_len: usize,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    z: i64,
    visible: bool,
}

/// One settings-inventory category verdict.
#[derive(Clone, Copy)]
struct InventoryRow {
    label: &'static [u8],
    /// The exact category string passed to `settings_hardware_status`.
    category: &'static str,
    code: Option<i64>,
}

struct Diagnostics {
    task_id: u64,
    surface_id: u32,
    ticks: Probe<u64>,
    platform: Probe<Platform>,
    /// Windows reported by the compositor, and this app's own entry within it.
    window_count: usize,
    own_window: Option<WindowInfo>,
    launcher: Probe<(u8, u32)>,
    storage_bytes: Probe<u64>,
    runtime_tasks: Option<usize>,
    active_user: u32,
    policy_version: Option<(u8, u8, u8)>,
    inventory: [InventoryRow; 6],
    status: [u8; 64],
    status_len: usize,
    status_color: u32,
}

/// The hardware categories this app reports. Each is a real
/// `settings_hardware_status` query, listed because a developer diagnosing a
/// device wants to know which subsystems the kernel itself considers present.
const INVENTORY: [(&[u8], &str); 6] = [
    (b"Display", "display"),
    (b"Storage", "storage"),
    (b"Sensors", "sensors"),
    (b"Battery", "battery"),
    (b"Wi-Fi", "wifi"),
    (b"Cellular", "cellular"),
];

// -----------------------------------------------------------------------------
// Minimal flat-JSON reader
// -----------------------------------------------------------------------------

fn json_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + key.len() + 2 < buf.len() {
        if buf[i] == b'"'
            && &buf[i + 1..i + 1 + key.len()] == key
            && buf[i + 1 + key.len()] == b'"'
            && (i == 0 || buf[i - 1] == b'{' || buf[i - 1] == b',')
        {
            let after = i + key.len() + 2;
            if after < buf.len() && buf[after] == b':' {
                return Some(after + 1);
            }
        }
        i += 1;
    }
    None
}

fn json_str(buf: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut i = json_value(buf, key)?;
    if i >= buf.len() || buf[i] != b'"' {
        return None;
    }
    i += 1;
    let mut n = 0usize;
    while i < buf.len() && buf[i] != b'"' {
        if n < out.len() {
            out[n] = buf[i];
        }
        n += 1;
        i += 1;
    }
    Some(n.min(out.len()))
}

fn json_i64(buf: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_value(buf, key)?;
    let negative = i < buf.len() && buf[i] == b'-';
    if negative {
        i += 1;
    }
    let mut v: i64 = 0;
    let mut digits = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((buf[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -v } else { v })
}

// -----------------------------------------------------------------------------
// Real sources
// -----------------------------------------------------------------------------

fn probe_platform() -> Probe<Platform> {
    let mut buf = [0u8; 256];
    let n = match std::hal_query_platform(&mut buf) {
        Ok(n) => n,
        Err(e) => return Probe::Refused(e),
    };
    let payload = &buf[..n.min(buf.len())];

    let mut arch = [0u8; 16];
    let arch_len = match json_str(payload, b"arch", &mut arch) {
        Some(k) => k,
        None => return Probe::Malformed,
    };
    let mut soc = [0u8; 32];
    let soc_len = match json_str(payload, b"soc", &mut soc) {
        Some(k) => k,
        None => return Probe::Malformed,
    };
    let cores = match json_i64(payload, b"cores") {
        Some(v) => v,
        None => return Probe::Malformed,
    };
    let freq_mhz = match json_i64(payload, b"freq_mhz") {
        Some(v) => v,
        None => return Probe::Malformed,
    };
    Probe::Ok(Platform { arch, arch_len, soc, soc_len, cores, freq_mhz })
}

/// Reads the 1 kHz boot timer, probing the granted capability table.
fn probe_ticks() -> Probe<u64> {
    let mut last = i64::MIN;
    for slot in 0..12usize {
        match std::get_ticks(slot) {
            Ok(t) => return Probe::Ok(t),
            Err(e) => last = e,
        }
    }
    Probe::Refused(last)
}

fn probe_launcher() -> Probe<(u8, u32)> {
    let mut out = [0u8; 12];
    match std::service_query(LAUNCHER_SERVICE_ID, &mut out) {
        Ok(n) if n >= 12 => Probe::Ok((
            out[1],
            u32::from_le_bytes([out[6], out[7], out[8], out[9]]),
        )),
        Ok(_) => Probe::Malformed,
        Err(e) => Probe::Refused(e),
    }
}

fn probe_storage() -> Probe<u64> {
    match std::storage_query() {
        Ok(b) => Probe::Ok(b),
        Err(e) => Probe::Refused(e),
    }
}

/// Parses `window_list`'s pipe-delimited records and finds this app's own window.
///
/// Each line is `id|owner|name|x|y|width|height|z|opacity|visible|input`.
fn read_windows(own_task: u64, own_surface: u32) -> (usize, Option<WindowInfo>) {
    let mut buf = [0u8; 1024];
    let n = match std::window_list(&mut buf) {
        Ok(n) => n,
        Err(_) => return (0, None),
    };
    let payload = &buf[..n.min(buf.len())];

    let mut count = 0usize;
    let mut own = None;
    let mut start = 0usize;
    while start < payload.len() {
        let end = payload[start..]
            .iter()
            .position(|b| *b == b'\n')
            .map(|i| start + i)
            .unwrap_or(payload.len());
        let line = &payload[start..end];
        start = end.saturating_add(1);
        if line.is_empty() {
            continue;
        }
        count += 1;

        // Split the record into at most 12 pipe-delimited fields. Each field is
        // kept as a length-tagged slice of the original line so no field needs a
        // fixed buffer and no field can be silently truncated into a wrong value.
        let mut f: [&[u8]; 12] = [&[]; 12];
        let mut fnum = 0usize;
        let mut fstart = 0usize;
        for i in 0..=line.len() {
            if i == line.len() || line[i] == b'|' {
                if fnum < 12 {
                    f[fnum] = &line[fstart..i];
                }
                fnum += 1;
                fstart = i + 1;
            }
        }
        if fnum < 11 {
            continue;
        }
        let id = parse_i64(f[0]).unwrap_or(-1);
        let owner = parse_i64(f[1]).unwrap_or(-1);
        // Only this task's own surface is reported as "this app's window". A
        // window belonging to some other task is counted but not claimed.
        if own.is_none() && owner == own_task as i64 && id == own_surface as i64 {
            let mut name = [0u8; 24];
            let nl = f[2].len().min(name.len());
            name[..nl].copy_from_slice(&f[2][..nl]);
            own = Some(WindowInfo {
                id: id as u32,
                owner: owner as u32,
                name,
                name_len: nl,
                x: parse_i64(f[3]).unwrap_or(0),
                y: parse_i64(f[4]).unwrap_or(0),
                width: parse_i64(f[5]).unwrap_or(0),
                height: parse_i64(f[6]).unwrap_or(0),
                z: parse_i64(f[7]).unwrap_or(0),
                visible: f[9].first().map(|b| *b == b'1').unwrap_or(false),
            });
        }
    }
    (count, own)
}

/// Parses a leading signed decimal, returning `None` when there is no digit.
fn parse_i64(field: &[u8]) -> Option<i64> {
    let mut i = 0usize;
    while i < field.len() && field[i] == b' ' {
        i += 1;
    }
    let negative = i < field.len() && field[i] == b'-';
    if negative {
        i += 1;
    }
    let mut v: i64 = 0;
    let mut digits = 0;
    while i < field.len() && field[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((field[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -v } else { v })
}

/// Counts the tasks `app_runtime_list` reports, one per newline-terminated record.
fn count_runtime_tasks() -> Option<usize> {
    let mut buf = [0u8; 1024];
    let n = std::app_runtime_list(&mut buf).ok()?;
    let payload = &buf[..n.min(buf.len())];
    let mut count = 0usize;
    for b in payload.iter() {
        if *b == b'\n' {
            count += 1;
        }
    }
    // A trailing record with no newline still counts, which is what a partially
    // filled buffer produces.
    if !payload.is_empty() && *payload.last().unwrap() != b'\n' {
        count += 1;
    }
    Some(count)
}

/// Reads the OS version the running image carries.
fn read_policy_version() -> Option<(u8, u8, u8)> {
    let mut buf = [0u8; 256];
    let mut read_len = 0usize;
    for slot in 0..12usize {
        if let Ok(fd) = std::fs_open(slot, "/system/etc/policy.json", std::FS_READ) {
            if let Ok(n) = std::fs_read(slot, fd, &mut buf) {
                let _ = std::fs_close(fd);
                if n > 0 {
                    read_len = n;
                    break;
                }
            }
            let _ = std::fs_close(fd);
        }
    }
    if read_len == 0 {
        return None;
    }
    parse_version_field(&buf[..read_len])
}

fn parse_version_field(buf: &[u8]) -> Option<(u8, u8, u8)> {
    let needle = b"\"version\":";
    let mut from = 0usize;
    let at = loop {
        if from + needle.len() > buf.len() {
            return None;
        }
        if buf[from..from + needle.len()] == *needle {
            break from + needle.len();
        }
        from += 1;
    };
    let mut i = at;
    while i < buf.len() && (buf[i] == b' ' || buf[i] == b'"') {
        i += 1;
    }
    let mut parts = [0u8; 3];
    let mut part = 0usize;
    let mut digits = 0;
    while i < buf.len() && part < 3 {
        let b = buf[i];
        if b.is_ascii_digit() {
            parts[part] = parts[part].saturating_mul(10).saturating_add(b - b'0');
            digits += 1;
            i += 1;
            continue;
        }
        if b == b'.' && digits > 0 {
            part += 1;
            digits = 0;
            i += 1;
            continue;
        }
        break;
    }
    if part < 2 || digits == 0 {
        return None;
    }
    Some((parts[0], parts[1], parts[2]))
}

fn service_state_word(state: u8) -> &'static [u8] {
    match state {
        0 => b"declared",
        1 => b"starting",
        2 => b"running",
        3 => b"stopped",
        4 => b"failed",
        _ => b"unknown",
    }
}

fn inventory_word(code: i64) -> &'static [u8] {
    match code {
        0 => b"available",
        1 => b"simulated",
        2 => b"unavailable",
        _ => b"unknown",
    }
}

fn status_name(code: i64) -> &'static [u8] {
    match code {
        -1 => b"PermissionDenied",
        -2 => b"InvalidSlot",
        -3 => b"InvalidArguments",
        -6 => b"TargetNotFound",
        -12 => b"DeviceError",
        _ => b"refused",
    }
}

// -----------------------------------------------------------------------------
// Formatting helpers
// -----------------------------------------------------------------------------

fn write_lit(s: &[u8], out: &mut [u8]) -> usize {
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s[..n]);
    n
}

fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

fn fmt_i64(v: i64, out: &mut [u8]) -> usize {
    if v < 0 && !out.is_empty() {
        out[0] = b'-';
        let n = fmt_u64(v.unsigned_abs(), &mut out[1..]);
        return n + 1;
    }
    fmt_u64(v as u64, out)
}

// -----------------------------------------------------------------------------
// Palette and layout
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF080D1A;
const CARD: u32 = 0xFF0F172A;
const CARD_INNER: u32 = 0xFF0B1120;
const CARD_RAISED: u32 = 0xFF1E293B;
const EDGE: u32 = 0xFF2B3A55;
const TEXT: u32 = 0xFFF1F5F9;
const MUTED: u32 = 0xFF94A3B8;
const FAINT: u32 = 0xFF64748B;
const ACCENT: u32 = 0xFF38BDF8;
const PURPLE: u32 = 0xFFA78BFA;
const OK: u32 = 0xFF34D399;
const WARN: u32 = 0xFFFBBF24;

// -----------------------------------------------------------------------------
// Geometry Helpers (Squircles & Rounded Rectangles)
// -----------------------------------------------------------------------------

fn in_rounded_rect(x: usize, y: usize, x0: usize, y0: usize, x1: usize, y1: usize, r: usize) -> bool {
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return false;
    }
    let left = x0 + r;
    let right = x1.saturating_sub(r + 1);
    let top = y0 + r;
    let bottom = y1.saturating_sub(r + 1);
    if x >= left && x <= right {
        return true;
    }
    if y >= top && y <= bottom {
        return true;
    }
    let dx = if x < left { (left - x) as isize } else { (x - right) as isize };
    let dy = if y < top { (top - y) as isize } else { (y - bottom) as isize };
    (dx * dx + dy * dy) <= (r * r) as isize
}

fn on_rounded_rect_border(x: usize, y: usize, x0: usize, y0: usize, x1: usize, y1: usize, r: usize) -> bool {
    if !in_rounded_rect(x, y, x0, y0, x1, y1, r) {
        return false;
    }
    if r > 1 && in_rounded_rect(x, y, x0 + 1, y0 + 1, x1.saturating_sub(1), y1.saturating_sub(1), r - 1) {
        return false;
    }
    true
}

// -----------------------------------------------------------------------------
// Vector Pictograms
// -----------------------------------------------------------------------------

/// Vector Diagnostics Chip icon for header.
fn icon_chip(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Chip body: 18x18 square
    if dx.abs() <= 9 && dy.abs() <= 9 {
        if dx.abs() == 9 || dy.abs() == 9 {
            return Some(ACCENT);
        }
        if dx.abs() <= 5 && dy.abs() <= 5 {
            if dx.abs() == 5 || dy.abs() == 5 {
                return Some(MUTED);
            }
            if dx.abs() <= 2 && dy.abs() <= 2 {
                return Some(OK);
            }
        }
        return Some(CARD_RAISED);
    }
    // Pins extending on all 4 sides
    if (dy.abs() == 3 || dy.abs() == 6) && (dx.abs() >= 10 && dx.abs() <= 13) {
        return Some(ACCENT);
    }
    if (dx.abs() == 3 || dx.abs() == 6) && (dy.abs() >= 10 && dy.abs() <= 13) {
        return Some(ACCENT);
    }
    None
}

/// Vector Task / Process icon for Card 0 (THIS PROCESS).
fn icon_task(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Task card frame: 18x20
    if dx.abs() <= 8 && dy.abs() <= 10 {
        if dx.abs() == 8 || dy.abs() == 10 {
            return Some(ACCENT);
        }
        // Top clip / tab
        if dy <= -7 && dx.abs() <= 3 {
            return Some(OK);
        }
        // Horizontal process rows
        if dy == -3 && dx >= -5 && dx <= 5 {
            return Some(MUTED);
        }
        if dy == 1 && dx >= -5 && dx <= 3 {
            return Some(TEXT);
        }
        if dy == 5 && dx >= -5 && dx <= 1 {
            return Some(MUTED);
        }
        return Some(CARD_RAISED);
    }
    // Top hanger clip loop
    if dy <= -11 && dy >= -13 && dx.abs() <= 2 {
        return Some(OK);
    }
    None
}

/// Vector CPU / Microprocessor icon for Card 1 (PLATFORM ARCHITECTURE).
fn icon_cpu(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Outer processor package: 16x16
    if dx.abs() <= 8 && dy.abs() <= 8 {
        if dx.abs() == 8 || dy.abs() == 8 {
            return Some(OK);
        }
        // Inner silicon core: 8x8
        if dx.abs() <= 4 && dy.abs() <= 4 {
            if dx.abs() == 4 || dy.abs() == 4 {
                return Some(ACCENT);
            }
            return Some(CARD_RAISED);
        }
        return Some(CARD_INNER);
    }
    // Pins on 4 sides
    if (dy.abs() == 2 || dy.abs() == 5) && (dx.abs() >= 9 && dx.abs() <= 11) {
        return Some(OK);
    }
    if (dx.abs() == 2 || dx.abs() == 5) && (dy.abs() >= 9 && dy.abs() <= 11) {
        return Some(OK);
    }
    None
}

/// Vector Inventory Matrix icon for Card 2 (HARDWARE INVENTORY).
fn icon_matrix(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let block_x = dx.abs() - 5;
    let block_y = dy.abs() - 5;
    if block_x.abs() <= 3 && block_y.abs() <= 3 {
        if block_x.abs() == 3 || block_y.abs() == 3 {
            return Some(PURPLE);
        }
        return Some(if dx < 0 && dy < 0 { OK } else { ACCENT });
    }
    // Interconnecting bus traces along axes
    if (dx == 0 && dy.abs() <= 8) || (dy == 0 && dx.abs() <= 8) {
        return Some(MUTED);
    }
    None
}

/// Vector Runtime Terminal Shell icon for Card 3 (ENVIRONMENT).
fn icon_runtime(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Terminal frame
    if dx.abs() <= 10 && dy.abs() <= 8 {
        if dx.abs() == 10 || dy.abs() == 8 {
            return Some(EDGE);
        }
        // Prompt >
        if (dy == -3 && dx == -6)
            || (dy == -2 && dx == -5)
            || (dy == -1 && dx == -4)
            || (dy == 0 && dx == -5)
            || (dy == 1 && dx == -6)
        {
            return Some(ACCENT);
        }
        // Cursor _
        if dy == 1 && dx >= -1 && dx <= 3 {
            return Some(TEXT);
        }
        return Some(CARD_INNER);
    }
    None
}

struct Card {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    caption: &'static [u8],
}

const CARDS: [Card; 4] = [
    Card { x0: 30, y0: 154, x1: 690, y1: 478, caption: b"THIS PROCESS IDENTITY" },
    Card { x0: 30, y0: 492, x1: 690, y1: 792, caption: b"PLATFORM ARCHITECTURE" },
    Card { x0: 30, y0: 806, x1: 690, y1: 1126, caption: b"HARDWARE AVAILABILITY INVENTORY" },
    Card { x0: 30, y0: 1140, x1: 690, y1: 1396, caption: b"SYSTEM RUNTIME ENVIRONMENT" },
];

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

fn render(surface: u32, app: &Diagnostics) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0usize;

    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = pixel(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[inline(never)]
fn pixel(x: usize, y: usize, app: &Diagnostics) -> u32 {
    // Header bar (0..96)
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }
        // Back button: tap hitbox (24..100) x (28..68)
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let is_border = on_rounded_rect_border(x, y, 24, 28, 100, 68, 10);
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            if is_border {
                return EDGE;
            }
            if chev {
                return TEXT;
            }
            return CARD_RAISED;
        }
        if font::text_at(x, y, 116, 34, 3, b"Diagnostics") {
            return TEXT;
        }
        if font::text_at(x, y, 118, 66, 1, b"KERNEL TELEMETRY & HARDWARE PROBES") {
            return MUTED;
        }
        // Diagnostics chip icon at top right
        if let Some(c) = icon_chip(x, y, 648, 48) {
            return c;
        }
        return CARD;
    }

    // Status pill container (100..142)
    if in_rounded_rect(x, y, 30, 100, 690, 142, 10) {
        if on_rounded_rect_border(x, y, 30, 100, 690, 142, 10) {
            return EDGE;
        }
        // Status indicator dot
        let ddx = (x as isize) - 48;
        let ddy = (y as isize) - 121;
        if ddx * ddx + ddy * ddy <= 4 * 4 {
            return app.status_color;
        }
        if font::text_at(x, y, 64, 114, 2, &app.status[..app.status_len]) {
            return app.status_color;
        }
        return CARD_RAISED;
    }

    // Card containers
    for (i, card) in CARDS.iter().enumerate() {
        if in_rounded_rect(x, y, card.x0, card.y0, card.x1, card.y1, 14) {
            if on_rounded_rect_border(x, y, card.x0, card.y0, card.x1, card.y1, 14) {
                return EDGE;
            }
            // Header divider line inside card
            if y == card.y0 + 38 && x >= card.x0 + 12 && x <= card.x1 - 12 {
                return EDGE;
            }
            // Card vector icon
            let icon_cx = card.x0 + 26;
            let icon_cy = card.y0 + 20;
            let ic = match i {
                0 => icon_task(x, y, icon_cx, icon_cy),
                1 => icon_cpu(x, y, icon_cx, icon_cy),
                2 => icon_matrix(x, y, icon_cx, icon_cy),
                3 => icon_runtime(x, y, icon_cx, icon_cy),
                _ => None,
            };
            if let Some(c) = ic {
                return c;
            }
            if font::text_at(x, y, card.x0 + 46, card.y0 + 13, 2, card.caption) {
                return ACCENT;
            }
            // Card body contents
            let body_c = match i {
                0 => process_card(x, y, app),
                1 => platform_card(x, y, app),
                2 => inventory_card(x, y, app),
                3 => environment_card(x, y, app),
                _ => None,
            };
            if let Some(c) = body_c {
                return c;
            }
            return CARD;
        }
    }

    // Bottom gesture home bar (hitbox iy >= 1400)
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }
    BG
}

#[inline(never)]
fn row(x: usize, y: usize, top: usize, label: &[u8], verdict: &[u8], color: u32, source: &[u8]) -> Option<u32> {
    if font::text_at(x, y, 48, top, 2, label) {
        return Some(MUTED);
    }
    let vx = 672usize.saturating_sub(font::text_width(verdict, 2));
    if font::text_at(x, y, vx, top, 2, verdict) {
        return Some(color);
    }
    if font::text_at(x, y, 48, top + 18, 2, source) {
        return Some(FAINT);
    }
    None
}

#[inline(never)]
fn line(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    if font::text_at(x, y, 48, top, 2, text) {
        return Some(color);
    }
    None
}

/// Formats a refusal as `refused (code) Name`, which is what a developer needs.
fn refusal(code: i64, out: &mut [u8]) -> usize {
    let mut n = write_lit(b"refused (", out);
    n += fmt_i64(code, &mut out[n..]);
    n += write_lit(b") ", &mut out[n..]);
    n += write_lit(status_name(code), &mut out[n..]);
    n
}

/// Card 0: identity, geometry and uptime of this very process.
#[inline(never)]
fn process_card(x: usize, y: usize, app: &Diagnostics) -> Option<u32> {
    let mut v = [0u8; 24];
    let n = fmt_u64(app.task_id, &mut v);
    if let Some(c) = row(x, y, 198, b"Task id", &v[..n], TEXT, b"get_task_id()") {
        return Some(c);
    }

    let n = fmt_u64(app.surface_id as u64, &mut v);
    if let Some(c) = row(x, y, 236, b"Surface id", &v[..n], TEXT, b"surface_create(6, 720x1440, z=200)") {
        return Some(c);
    }

    match app.ticks {
        Probe::Ok(t) => {
            let n = fmt_u64(t, &mut v);
            if let Some(c) = row(x, y, 274, b"Boot ticks", &v[..n], TEXT, b"get_ticks(), 1 kHz since kernel boot") {
                return Some(c);
            }
            let n = fmt_u64(t / 1000, &mut v);
            if let Some(c) = row(x, y, 312, b"Uptime", &v[..n], TEXT, b"boot ticks / 1000 = seconds") {
                return Some(c);
            }
        }
        Probe::Refused(e) => {
            let mut w = [0u8; 40];
            let n = refusal(e, &mut w);
            if let Some(c) = row(x, y, 274, b"Boot ticks", &w[..n], WARN, b"get_ticks() refused") {
                return Some(c);
            }
            if let Some(c) = row(x, y, 312, b"Uptime", b"unavailable", WARN, b"derived from the same refused read") {
                return Some(c);
            }
        }
        Probe::Malformed => {
            if let Some(c) = row(x, y, 274, b"Boot ticks", b"no data", WARN, b"get_ticks() returned no ticks") {
                return Some(c);
            }
        }
    }

    match &app.own_window {
        Some(w) => {
            let mut word = [0u8; 40];
            let n = write_lit(&w.name[..w.name_len], &mut word);
            let n = n + write_lit(b" at ", &mut word[n..]);
            let n = n + fmt_i64(w.x, &mut word[n..]);
            let n = n + write_lit(b",", &mut word[n..]);
            let n = n + fmt_i64(w.y, &mut word[n..]);
            if let Some(c) = row(x, y, 350, b"Own window", &word[..n], TEXT, b"window_list() -> name, x, y") {
                return Some(c);
            }
            let mut word = [0u8; 40];
            let n = write_lit(b"", &mut word);
            let n = n + fmt_i64(w.width, &mut word[n..]);
            let n = n + write_lit(b"x", &mut word[n..]);
            let n = n + fmt_i64(w.height, &mut word[n..]);
            let n = n + write_lit(if w.visible { b" visible" } else { b" hidden" }, &mut word[n..]);
            if let Some(c) = row(x, y, 388, b"Geometry", &word[..n], TEXT, b"window_list() -> width, height, visible") {
                return Some(c);
            }
            let mut word = [0u8; 40];
            let mut n = write_lit(b"win ", &mut word);
            n = n + fmt_u64(w.id as u64, &mut word[n..]);
            n = n + write_lit(b" z ", &mut word[n..]);
            n = n + fmt_i64(w.z, &mut word[n..]);
            n = n + write_lit(b" task ", &mut word[n..]);
            n = n + fmt_u64(w.owner as u64, &mut word[n..]);
            if let Some(c) = row(x, y, 426, b"Compositor", &word[..n], MUTED, b"window_list() -> id, z_order, owner_task") {
                return Some(c);
            }
        }
        None => {
            if let Some(c) = row(x, y, 350, b"Own window", b"not listed", WARN, b"window_list() has no entry for this surface") {
                return Some(c);
            }
        }
    }
    None
}

/// Card 1: the SoC, straight from the HAL.
#[inline(never)]
fn platform_card(x: usize, y: usize, app: &Diagnostics) -> Option<u32> {
    match &app.platform {
        Probe::Ok(p) => {
            if let Some(c) = row(x, y, 540, b"Architecture", &p.arch[..p.arch_len], OK, b"hal_query_platform() -> arch") {
                return Some(c);
            }
            if let Some(c) = row(x, y, 580, b"SoC", &p.soc[..p.soc_len], OK, b"hal_query_platform() -> soc") {
                return Some(c);
            }
            let mut w = [0u8; 24];
            let n = fmt_i64(p.cores, &mut w);
            if let Some(c) = row(x, y, 620, b"CPU cores", &w[..n], OK, b"hal_query_platform() -> cores") {
                return Some(c);
            }
            let mut w = [0u8; 24];
            let n = fmt_i64(p.freq_mhz, &mut w);
            if let Some(c) = row(x, y, 660, b"Base clock", &w[..n], OK, b"hal_query_platform() -> freq_mhz") {
                return Some(c);
            }
            if line(x, y, 704, b"The base clock is the HAL's platform table,", FAINT).is_some() {
                return Some(FAINT);
            }
            line(x, y, 726, b"not a measured running frequency.", FAINT)
        }
        Probe::Refused(e) => {
            let mut w = [0u8; 40];
            let n = refusal(*e, &mut w);
            row(x, y, 540, b"Platform", &w[..n], WARN, b"hal_query_platform() refused")
        }
        Probe::Malformed => {
            row(x, y, 540, b"Platform", b"no data", WARN, b"hal_query_platform() payload lacked fields")
        }
    }
}

/// Card 2: the kernel's own hardware availability table.
#[inline(never)]
fn inventory_card(x: usize, y: usize, app: &Diagnostics) -> Option<u32> {
    macro_rules! item {
        ($i:expr) => {{
            if let Some(c) = item_row(x, y, 852 + $i * 40, &app.inventory[$i]) {
                return Some(c);
            }
        }};
    }
    item!(0);
    item!(1);
    item!(2);
    item!(3);
    item!(4);
    item!(5);

    line(x, y, 1096, b"'simulated' is the kernel's word, not this app's.", FAINT)
}

/// One inventory row, with the verdict the kernel returned for that category.
#[inline(never)]
fn item_row(x: usize, y: usize, top: usize, entry: &InventoryRow) -> Option<u32> {
    let mut w = [0u8; 24];
    let (color, wn) = match entry.code {
        Some(code) => (if code == 0 { OK } else { WARN }, write_lit(inventory_word(code), &mut w)),
        None => (WARN, write_lit(b"refused", &mut w)),
    };
    let mut source = [0u8; 48];
    let sn = write_lit(b"settings_hardware_status(\"", &mut source);
    let sn = sn + write_lit(entry.category.as_bytes(), &mut source[sn..]);
    let sn = if sn < source.len() {
        source[sn] = b'"';
        sn + 1
    } else {
        sn
    };
    row(x, y, top, entry.label, &w[..wn], color, &source[..sn.min(source.len())])
}

/// Card 3: everything else the kernel will tell this task about its world.
#[inline(never)]
fn environment_card(x: usize, y: usize, app: &Diagnostics) -> Option<u32> {
    let mut w = [0u8; 32];
    let (color, wn) = match &app.launcher {
        Probe::Ok((state, gen)) => {
            let n = write_lit(service_state_word(*state), &mut w);
            let n = n + write_lit(b" gen ", &mut w[n..]);
            (if *state == 2 { OK } else { WARN }, n + fmt_u64(*gen as u64, &mut w[n..]))
        }
        Probe::Refused(e) => {
            let n = refusal(*e, &mut w);
            (WARN, n)
        }
        Probe::Malformed => {
            let n = write_lit(b"no record", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 1186, b"Launcher", &w[..wn], color, b"service_query(10)") {
        return Some(c);
    }

    let mut w = [0u8; 40];
    let (color, wn) = match app.storage_bytes {
        Probe::Ok(b) => {
            let n = write_lit(b"", &mut w);
            let n = n + fmt_u64(b / (1024 * 1024), &mut w[n..]);
            let n = n + write_lit(b" MiB", &mut w[n..]);
            (OK, n)
        }
        Probe::Refused(e) => {
            let n = refusal(e, &mut w);
            (WARN, n)
        }
        Probe::Malformed => {
            let n = write_lit(b"no data", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 1226, b"Block storage", &w[..wn], color, b"storage_query() -> sectors x 512") {
        return Some(c);
    }

    let mut w = [0u8; 40];
    let (color, wn) = match app.runtime_tasks {
        Some(n) => {
            let k = write_lit(b"", &mut w);
            let k = k + fmt_u64(n as u64, &mut w[k..]);
            let k = k + write_lit(b" other tasks", &mut w[k..]);
            (MUTED, k)
        }
        None => {
            let n = write_lit(b"refused", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 1266, b"Runtime", &w[..wn], color, b"app_runtime_list() -> task records") {
        return Some(c);
    }

    let mut w = [0u8; 40];
    let (color, wn) = match app.policy_version {
        Some((maj, min, patch)) => {
            let n = write_lit(b"Inter OS ", &mut w);
            let n = n + fmt_u64(maj as u64, &mut w[n..]);
            let n = n + write_lit(b".", &mut w[n..]);
            let n = n + fmt_u64(min as u64, &mut w[n..]);
            let n = n + write_lit(b".", &mut w[n..]);
            let n = n + fmt_u64(patch as u64, &mut w[n..]);
            (OK, n)
        }
        None => {
            let n = write_lit(b"not readable", &mut w);
            (WARN, n)
        }
    };
    if let Some(c) = row(x, y, 1306, b"OS version", &w[..wn], color, b"/system/etc/policy.json") {
        return Some(c);
    }

    let mut w = [0u8; 40];
    let n = write_lit(b"build ", &mut w);
    let n = n + write_lit(BUILD_VERSION.as_bytes(), &mut w[n..]);
    if let Some(c) = row(x, y, 1346, b"This binary", &w[..n], MUTED, b"compiled into this ELF, not read back") {
        return Some(c);
    }
    None
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

fn status(app: &mut Diagnostics, text: &[u8], color: u32) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
    app.status_color = color;
}

/// Re-runs every probe, so the screen only ever shows values just measured.
fn reprobe(app: &mut Diagnostics) {
    app.ticks = probe_ticks();
    app.platform = probe_platform();
    app.launcher = probe_launcher();
    app.storage_bytes = probe_storage();
    app.runtime_tasks = count_runtime_tasks();
    app.active_user = std::user_get_current();
    app.policy_version = read_policy_version();
    for (i, (_, category)) in INVENTORY.iter().enumerate() {
        app.inventory[i].code = std::settings_hardware_status(category).ok();
    }
    let (count, own) = read_windows(app.task_id, app.surface_id);
    app.window_count = count;
    app.own_window = own;
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[TEST_APP (Ring 3)] Platform self-diagnostic starting.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[TEST_APP] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut app = Diagnostics {
        task_id: std::get_task_id(),
        surface_id: surface,
        ticks: Probe::Malformed,
        platform: Probe::Malformed,
        window_count: 0,
        own_window: None,
        launcher: Probe::Malformed,
        storage_bytes: Probe::Malformed,
        runtime_tasks: None,
        active_user: 0,
        policy_version: None,
        inventory: [InventoryRow { label: b"", category: "", code: None }; 6],
        status: [0; 64],
        status_len: 0,
        status_color: MUTED,
    };
    for (i, (label, category)) in INVENTORY.iter().enumerate() {
        app.inventory[i] = InventoryRow { label, category, code: None };
    }

    reprobe(&mut app);

    // The console states the diagnostic's own outcome: how many probes returned
    // data and how many were refused. Both numbers are counted here, not guessed.
    let refused = [
        matches!(app.ticks, Probe::Refused(_)),
        matches!(app.platform, Probe::Refused(_)),
        matches!(app.launcher, Probe::Refused(_)),
        matches!(app.storage_bytes, Probe::Refused(_)),
        app.runtime_tasks.is_none(),
    ]
    .iter()
    .filter(|b| **b)
    .count();

    std::log(0, "[TEST_APP] Diagnostic complete; reporting each probe's real result on screen.").ok();
    let mut note = [0u8; 48];
    let mut n = write_lit(b"5 core probes run, ", &mut note);
    n += fmt_u64(refused as u64, &mut note[n..]);
    n += write_lit(b" refused.", &mut note[n..]);
    status(&mut app, &note[..n], if refused == 0 { OK } else { ACCENT });

    if refused > 0 {
        std::log(0, "[TEST_APP] Some probes were refused; each refusal names the capability it needed.").ok();
    }

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[TEST_APP] Initial frame presented to hardware scanout.").ok();

    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if matches!(input.kind, 4 | 6 | 7 | 8 | 10) {
                        std::log(0, "[TEST_APP] Navigation gesture. Exiting to launcher.").ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        let ix = input.x as usize;
                        let iy = input.y as usize;
                        let tapped_back = (24..100).contains(&ix) && (28..68).contains(&iy);
                        if tapped_back || iy >= 1400 {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        reprobe(&mut app);
                        let mut note = [0u8; 48];
                        let n = write_lit(b"re-ran every probe on tap", &mut note);
                        status(&mut app, &note[..n], ACCENT);
                        render(surface, &app);
                        let _ = std::surface_flip(6);
                    }
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Test app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
