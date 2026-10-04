//! InterOS Core Open-Source Reference App: Hello World
//!
//! The smallest thing that can be correct.
//!
//! This app deliberately has no device data source. It does not sample a
//! sensor, open a file of its own, or invent a reading. What it does is prove
//! that the Ring-3 launch path works end to end, and it shows only values it
//! read from a real kernel source, each one labelled with that source:
//!
//! - `surface_create` / `surface_flip`   -> the DRM/KMS client surface really allocated
//! - `get_task_id`                        -> the task id the kernel scheduler assigned
//! - `get_ticks`                          -> 1 kHz hardware timer ticks since kernel boot
//! - `service_query(10)`                  -> live launcher (service 10) lifecycle record
//! - `/system/etc/policy.json`            -> the OS version string the image ships
//!
//! Anything that fails to read is reported as unavailable with the reason, not
//! substituted with a plausible-looking constant.

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
/// This is a build-time constant of the artifact itself, which is a real
/// source: it describes exactly the ELF that is running. It is not, and is not
/// presented as, a version read back from the kernel at runtime.
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// Result of one real probe. Nothing here is ever defaulted to a made-up value.
enum Probe<T: Copy> {
    Ok(T),
    /// The syscall returned a negative kernel status code.
    Refused(i64),
    /// The syscall succeeded but the payload did not contain the field we need.
    Malformed,
}

struct HelloWorld {
    task_id: u64,
    ticks: Probe<u64>,
    surface_id: u32,
    launcher: Probe<LauncherRecord>,
    policy_version: Probe<PolicyVersion>,
}

/// Decoded 12-byte `service_query` ABI record.
#[derive(Clone, Copy)]
struct LauncherRecord {
    state: u8,
    abi_version: u16,
    generation: u32,
    critical: bool,
}

#[derive(Clone, Copy)]
struct PolicyVersion {
    major: u8,
    minor: u8,
    patch: u8,
}

// -----------------------------------------------------------------------------
// Real sources
// -----------------------------------------------------------------------------

/// Reads the 12-byte service record for `service_id`.
///
/// `service_query` is a discovery syscall with no ambient authority, so this is
/// a genuine read of live kernel service state.
fn probe_service(service_id: u8) -> Probe<LauncherRecord> {
    let mut out = [0u8; 12];
    match std::service_query(service_id, &mut out) {
        Ok(n) if n >= 12 => Probe::Ok(LauncherRecord {
            state: out[1],
            abi_version: u16::from_le_bytes([out[4], out[5]]),
            generation: u32::from_le_bytes([out[6], out[7], out[8], out[9]]),
            critical: out[3] != 0,
        }),
        Ok(_) => Probe::Malformed,
        Err(e) => Probe::Refused(e),
    }
}

/// Reads the 1 kHz boot timer.
///
/// The capability slot for `TimerTickRead` is chosen by the spawn policy, so
/// probe the granted table rather than assume an index. A refused read stays
/// refused: it is reported as unavailable instead of being reported as zero
/// seconds of uptime.
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

/// Reads `/system/etc/policy.json` and extracts the `"version"` field.
///
/// The read-only system policy file is seeded into the VFS by the kernel at
/// boot, so this is the version the running image actually carries.
fn probe_policy_version() -> Probe<PolicyVersion> {
    let mut buf = [0u8; 256];
    let mut read_len = 0usize;

    // The capability slot an app receives for filesystem reads is chosen by the
    // spawn policy, not by this app, so probe the table rather than assume one.
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
        return Probe::Refused(-1);
    }

    match parse_version_field(&buf[..read_len]) {
        Some(v) => Probe::Ok(v),
        None => Probe::Malformed,
    }
}

/// Extracts the number after the `"version"` key of a flat JSON object.
///
/// WHY hand-parsed and not assumed: the payload is produced by the kernel's
/// `core::fmt` writer, so this only has to understand the exact flat shape the
/// kernel emits. If the shape ever changes the parse fails and the UI says so
/// rather than inventing a version.
fn parse_version_field(buf: &[u8]) -> Option<PolicyVersion> {
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
    let mut digits = 0usize;
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
    Some(PolicyVersion { major: parts[0], minor: parts[1], patch: parts[2] })
}

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
const EDGE: u32 = 0xFF26354D;
const TEXT: u32 = 0xFFF4F7FB;
const MUTED: u32 = 0xFF9AA8BC;
const ACCENT: u32 = 0xFF31C4FF;
const OK: u32 = 0xFF56D68A;
const WARN: u32 = 0xFFFFC043;

/// A bordered content card with a caption.
struct Card {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    caption: &'static [u8],
    caption_color: u32,
}

const CARDS: [Card; 4] = [
    Card { x0: 30, y0: 118, x1: 690, y1: 372, caption: b"THIS APP", caption_color: ACCENT },
    Card { x0: 30, y0: 392, x1: 690, y1: 592, caption: b"OS IDENTITY (READ AT RUNTIME)", caption_color: MUTED },
    Card { x0: 30, y0: 612, x1: 690, y1: 1012, caption: b"LAUNCH PATH, MEASURED", caption_color: MUTED },
    Card { x0: 30, y0: 1032, x1: 690, y1: 1300, caption: b"SCOPE", caption_color: MUTED },
];

/// Small-caps field label on the left, value on the right, provenance underneath.
struct Row {
    y: u32,
    label: &'static [u8],
    value: [u8; 24],
    value_len: usize,
    value_color: u32,
    source: [u8; 48],
    source_len: usize,
}

impl Row {
    const fn new(y: u32, label: &'static [u8], source: &'static [u8]) -> Self {
        let mut source_buf = [0u8; 48];
        let mut i = 0usize;
        while i < source.len() && i < source_buf.len() {
            source_buf[i] = source[i];
            i += 1;
        }
        Self { y, label, value: [0; 24], value_len: 0, value_color: TEXT, source: source_buf, source_len: i }
    }

    fn set(&mut self, text: &[u8], color: u32) {
        let n = text.len().min(self.value.len());
        self.value[..n].copy_from_slice(&text[..n]);
        self.value_len = n;
        self.value_color = color;
    }
}

/// Writes the decimal digits of `v` into `out`, most significant first.
/// Returns the number of bytes written. Never invents a value: `v` is the
/// measured integer, and zero prints as a single `0`.
fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}

/// Decimal digits of a signed kernel status code, prefixed with `-` when negative.
fn fmt_i64(v: i64, out: &mut [u8]) -> usize {
    if v < 0 && !out.is_empty() {
        out[0] = b'-';
        let n = fmt_u64(v.unsigned_abs(), &mut out[1..]);
        return n + 1;
    }
    fmt_u64(v as u64, out)
}

fn write_lit(s: &[u8], out: &mut [u8]) -> usize {
    let n = s.len().min(out.len());
    out[..n].copy_from_slice(&s[..n]);
    n
}

fn draw_rows(x: usize, y: usize, rows: &[Row], base: usize) -> Option<u32> {
    for row in rows {
        let top = row.y as usize;
        if font::text_at(x, y, base + 14, top, 2, row.label) {
            return Some(MUTED);
        }
        let vx = 690usize.saturating_sub(14 + font::text_width(&row.value[..row.value_len], 2));
        if font::text_at(x, y, vx, top, 2, &row.value[..row.value_len]) {
            return Some(row.value_color);
        }
        if font::text_at(x, y, base + 14, top + 18, 2, &row.source[..row.source_len]) {
            return Some(0xFF6C7A8E);
        }
    }
    None
}

fn render(surface: u32, app: &HelloWorld) {
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
fn pixel(x: usize, y: usize, app: &HelloWorld) -> u32 {
    // Header
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let border = y == 28 || y == 67 || x == 24 || x == 99;
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            return if border || chev { TEXT } else { CARD_RAISED };
        }
        if font::text_at(x, y, 116, 32, 3, b"Hello World") {
            return TEXT;
        }
        return CARD;
    }

    // Cards
    for card in CARDS.iter() {
        if y >= card.y0 && y < card.y1 && x >= card.x0 && x < card.x1 {
            if y == card.y0 || y == card.y1 - 1 || x == card.x0 || x == card.x1 - 1 {
                return EDGE;
            }
            if font::text_at(x, y, card.x0 + 14, card.y0 + 12, 2, card.caption) {
                return card.caption_color;
            }
            return CARD;
        }
    }

    // Card 1: identity of this binary.
    if (170..372).contains(&y) && (30..690).contains(&x) {
        if font::text_at(x, y, 48, 196, 5, b"Hello World") {
            return TEXT;
        }
        let mut v = [0u8; 24];
        let n = write_lit(b"build ", &mut v);
        let dn = write_lit(BUILD_VERSION.as_bytes(), &mut v[n..]);
        if font::text_at(x, y, 48, 300, 2, &v[..n + dn]) {
            return ACCENT;
        }
        if font::text_at(x, y, 48, 330, 2, b"compiled into this ELF") {
            return MUTED;
        }
        if font::text_at(x, y, 48, 236, 2, b"Ring 3 user mode") {
            return MUTED;
        }
        return CARD;
    }

    // Card 2: OS identity read back from the running image.
    if (440..592).contains(&y) && (30..690).contains(&x) {
        let mut v = [0u8; 24];
        match &app.policy_version {
            Probe::Ok(p) => {
                // Dotted, exactly as the file spells it. Collapsing the three
                // numbers into one integer would print "300" for 0.3.0, which is
                // a version string the image never contained.
                let n = write_lit(b"Inter OS ", &mut v);
                let n = n + fmt_u64(p.major as u64, &mut v[n..]);
                let n = n + write_lit(b".", &mut v[n..]);
                let n = n + fmt_u64(p.minor as u64, &mut v[n..]);
                let n = n + write_lit(b".", &mut v[n..]);
                let n = n + fmt_u64(p.patch as u64, &mut v[n..]);
                if font::text_at(x, y, 48, 462, 4, &v[..n]) {
                    return TEXT;
                }
            }
            Probe::Refused(e) => {
                let n = write_lit(b"unavailable (status ", &mut v);
                let dn = fmt_i64(*e, &mut v[n..]);
                let total = n + dn;
                if total <= v.len() {
                    v[total] = b')';
                }
                if font::text_at(x, y, 48, 462, 3, &v[..total]) {
                    return WARN;
                }
            }
            Probe::Malformed => {
                if font::text_at(x, y, 48, 462, 3, b"no version field in file") {
                    return WARN;
                }
            }
        }
        let src = match &app.policy_version {
            Probe::Ok(_) => b"/system/etc/policy.json".as_slice(),
            Probe::Refused(_) => b"not readable by this app".as_slice(),
            Probe::Malformed => b"/system/etc/policy.json".as_slice(),
        };
        if font::text_at(x, y, 48, 540, 2, src) {
            return MUTED;
        }
        return CARD;
    }

    // Card 3: what the launch path actually produced.
    if (660..1012).contains(&y) && (30..690).contains(&x) {
        let mut r_task = Row::new(672, b"Task id", b"get_task_id()");
        let mut v = [0u8; 24];
        let n = fmt_u64(app.task_id, &mut v);
        r_task.set(&v[..n], TEXT);

        let mut r_surface = Row::new(736, b"Surface", b"surface_create(6, 720x1440, z=200)");
        let n = fmt_u64(app.surface_id as u64, &mut v);
        r_surface.set(&v[..n], TEXT);

        let mut r_ticks = Row::new(800, b"Boot ticks", b"get_ticks(), 1 kHz since kernel boot");
        let mut r_up = Row::new(864, b"Uptime", b"boot ticks / 1000 = seconds");
        match app.ticks {
            Probe::Ok(t) => {
                let n = fmt_u64(t, &mut v);
                r_ticks.set(&v[..n], TEXT);
                let n = fmt_u64(t / 1000, &mut v);
                r_up.set(&v[..n], TEXT);
            }
            Probe::Refused(e) => {
                // The refused status belongs on the row that made the call. Uptime
                // is derived from the same read, so it gets the word, not the code:
                // a bare "-1" in an uptime row would read as a measurement.
                let n = write_lit(b"refused (", &mut v);
                let mut n = n + fmt_i64(e, &mut v[n..]);
                if n < v.len() {
                    v[n] = b')';
                    n += 1;
                }
                r_ticks.set(&v[..n], WARN);
                r_up.set(b"unavailable", WARN);
            }
            Probe::Malformed => {
                r_ticks.set(b"no data", WARN);
                r_up.set(b"no data", WARN);
            }
        }

        let mut r_svc = Row::new(928, b"Launcher", b"service_query(10)");
        let mut word = [0u8; 24];
        let mut src = [0u8; 48];
        match &app.launcher {
            Probe::Ok(l) => {
                let n = write_lit(service_state_word(l.state), &mut word);
                r_svc.set(&word[..n], if l.state == 2 { OK } else { WARN });
                let sn = write_lit(b"kernel service 10, abi=", &mut src);
                let sn = sn + fmt_u64(l.abi_version as u64, &mut src[sn..]);
                let sn = sn + write_lit(b" gen=", &mut src[sn..]);
                let sn = sn + fmt_u64(l.generation as u64, &mut src[sn..]);
                let sn = if l.critical { sn + write_lit(b" critical", &mut src[sn..]) } else { sn };
                r_svc.source_len = sn.min(src.len());
            }
            Probe::Refused(e) => {
                let n = write_lit(b"refused (", &mut word);
                let mut n = n + fmt_i64(*e, &mut word[n..]);
                if n < word.len() {
                    word[n] = b')';
                    n += 1;
                }
                r_svc.set(&word[..n], WARN);
            }
            Probe::Malformed => {
                r_svc.set(b"no record", WARN);
            }
        }

        let rows = [r_task, r_surface, r_ticks, r_up, r_svc];
        if let Some(c) = draw_rows(x, y, &rows, 30) {
            return c;
        }
        return CARD;
    }

    // Card 4: scope, stated plainly.
    if (1080..1300).contains(&y) && (30..690).contains(&x) {
        let lines: [&[u8]; 6] = [
            b"No sensor, camera, radio or",
            b"paired device is bound to this",
            b"platform, so this app has no",
            b"measurement of its own to show.",
            b"It reports kernel facts only, and",
            b"labels where each one came from.",
        ];
        for (i, line) in lines.iter().enumerate() {
            let top = 1080 + 18 + i * 30;
            if font::text_at(x, y, 48, top, 2, line) {
                return TEXT;
            }
        }
        return CARD;
    }

    // Home gesture pill
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }
    BG
}

fn service_state_word(state: u8) -> &'static [u8] {
    // ServiceState: Declared=0, Starting=1, Running=2, Stopped=3, Failed=4.
    match state {
        0 => b"declared",
        1 => b"starting",
        2 => b"running",
        3 => b"stopped",
        4 => b"failed",
        _ => b"unknown",
    }
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[HELLO WORLD (Ring 3)] Launch-path smoke test starting.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[HELLO WORLD] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let app = HelloWorld {
        task_id: std::get_task_id(),
        ticks: probe_ticks(),
        surface_id: surface,
        launcher: probe_service(LAUNCHER_SERVICE_ID),
        policy_version: probe_policy_version(),
    };

    match &app.policy_version {
        Probe::Ok(v) => {
            std::log(0, "[HELLO WORLD] Read OS version from /system/etc/policy.json.").ok();
            let _ = v;
        }
        Probe::Refused(e) => {
            std::log(
                0,
                "[HELLO WORLD] /system/etc/policy.json is not readable by this app; reporting unavailable.",
            )
            .ok();
            let _ = e;
        }
        Probe::Malformed => {
            std::log(0, "[HELLO WORLD] policy.json carried no version field; reporting unavailable.").ok();
        }
    }

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[HELLO WORLD] Initial frame presented to hardware scanout.").ok();

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
                    let gesture_exit = matches!(input.kind, 4 | 6 | 7 | 8 | 10);
                    let tapped_back =
                        input.kind == 1 && ((24..100).contains(&input.x) && (28..68).contains(&input.y));
                    let tapped_home = input.kind == 1 && input.y >= 1400;
                    if gesture_exit || tapped_back || tapped_home {
                        std::log(0, "[HELLO WORLD] Navigation gesture recognized. Exiting to launcher.").ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Hello World app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
