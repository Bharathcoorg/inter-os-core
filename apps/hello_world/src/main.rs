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
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// Result of one real probe.
enum Probe<T: Copy> {
    Ok(T),
    Refused(i64),
    Malformed,
}

struct HelloWorld {
    task_id: u64,
    ticks: Probe<u64>,
    surface_id: u32,
    launcher: Probe<LauncherRecord>,
    policy_version: Probe<PolicyVersion>,
}

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

fn probe_policy_version() -> Probe<PolicyVersion> {
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
        return Probe::Refused(-1);
    }

    match parse_version_field(&buf[..read_len]) {
        Some(v) => Probe::Ok(v),
        None => Probe::Malformed,
    }
}

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
// Palette and geometry
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
const CARD_INNER: u32 = 0xFF0B121E;
const EDGE: u32 = 0xFF26354D;
const TEXT: u32 = 0xFFF4F7FB;
const MUTED: u32 = 0xFF9AA8BC;
const FAINT: u32 = 0xFF6C7A8E;
const ACCENT: u32 = 0xFF31C4FF;
const OK: u32 = 0xFF56D68A;
const WARN: u32 = 0xFFFFC043;
const AMBER: u32 = 0xFFF59E0B;

#[inline(always)]
fn in_rounded_rect(x: usize, y: usize, x0: usize, y0: usize, x1: usize, y1: usize, r: usize) -> bool {
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return false;
    }
    let left = x0 + r;
    let right = x1.saturating_sub(r + 1);
    let top = y0 + r;
    let bottom = y1.saturating_sub(r + 1);
    let dx = if x < left { left - x } else if x > right { x - right } else { 0 };
    let dy = if y < top { top - y } else if y > bottom { y - bottom } else { 0 };
    dx * dx + dy * dy <= r * r
}

#[inline(always)]
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

/// Vector Globe / Planet pictogram for header.
fn icon_globe(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq <= 16 * 16 {
        if dist_sq >= 14 * 14 {
            return Some(ACCENT);
        }
        // Equator and central meridian lines
        if dy.abs() <= 1 || dx.abs() <= 1 {
            return Some(MUTED);
        }
        // Latitude / Longitude ellipse
        let el = dx * dx * 2 + dy * dy;
        if el >= 9 * 9 && el <= 12 * 12 {
            return Some(FAINT);
        }
        return Some(CARD_INNER);
    }
    None
}

/// Vector Sparkle / Star icon for Card 0.
fn icon_star(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let abs_x = dx.abs();
    let abs_y = dy.abs();
    // 4-pointed star
    if (abs_x <= 8 && abs_y <= 1) || (abs_y <= 8 && abs_x <= 1) || (abs_x + abs_y <= 6) {
        return Some(AMBER);
    }
    None
}

/// Vector OS Shield / System icon for Card 1.
fn icon_os(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dx.abs() <= 8 && dy >= -7 && dy <= 8 {
        let is_border = dy == -7 || dx.abs() == 8 || (dy >= 4 && (dx.abs() as isize + dy) >= 12);
        if is_border {
            return Some(ACCENT);
        }
        if dx.abs() <= 4 && dy.abs() <= 3 {
            return Some(TEXT);
        }
        return Some(CARD_INNER);
    }
    None
}

/// Vector Launch Speedometer / Gauge icon for Card 2.
fn icon_speed(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq >= 6 * 6 && dist_sq <= 9 * 9 && dy <= 3 {
        return Some(OK);
    }
    // Needle pointing to top-right
    if dx >= 0 && dy <= 0 && (dx + dy).abs() <= 1 && dx <= 5 {
        return Some(WARN);
    }
    None
}

/// Vector Document / Truth icon for Card 3.
fn icon_verified(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dx.abs() <= 8 && dy.abs() <= 8 {
        if dx.abs() == 8 || dy.abs() == 8 {
            return Some(MUTED);
        }
        // Checkmark inside
        if (dx == -3 && dy == 0)
            || (dx == -2 && dy == 1)
            || (dx == -1 && dy == 2)
            || (dx == 0 && dy == 1)
            || (dx == 1 && dy == 0)
            || (dx == 2 && dy == -1)
            || (dx == 3 && dy == -2)
        {
            return Some(OK);
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
    caption_color: u32,
}

const CARDS: [Card; 4] = [
    Card { x0: 30, y0: 114, x1: 690, y1: 372, caption: b"APPLICATION IDENTITY", caption_color: ACCENT },
    Card { x0: 30, y0: 390, x1: 690, y1: 592, caption: b"OS DISTRIBUTION & IDENTITY", caption_color: ACCENT },
    Card { x0: 30, y0: 610, x1: 690, y1: 1012, caption: b"MEASURED LAUNCH PATH & TIMINGS", caption_color: ACCENT },
    Card { x0: 30, y0: 1030, x1: 690, y1: 1300, caption: b"PLATFORM REALITY CONTRACT", caption_color: ACCENT },
];

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
            return Some(FAINT);
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
    // Header (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }

        // Back button
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let on_border = on_rounded_rect_border(x, y, 24, 28, 100, 68, 8);
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            return if on_border || chev { TEXT } else { CARD_RAISED };
        }

        // Globe Icon
        if let Some(c) = icon_globe(x, y, 134, 48) {
            return c;
        }

        // App Title and Subtitle
        if font::text_at(x, y, 164, 32, 3, b"Hello World") {
            return TEXT;
        }
        if font::text_at(x, y, 166, 62, 1, b"RING 3 REFERENCE BENCHMARK") {
            return FAINT;
        }

        // Live status pill on top right
        if in_rounded_rect(x, y, 540, 34, 690, 62, 6) {
            if on_rounded_rect_border(x, y, 540, 34, 690, 62, 6) {
                return EDGE;
            }
            let (dx, dy) = (x as isize - 556, y as isize - 48);
            if dx * dx + dy * dy <= 4 * 4 {
                return OK;
            }
            if font::text_at(x, y, 568, 41, 2, b"RUNNING") {
                return OK;
            }
            return CARD_RAISED;
        }

        return CARD;
    }

    // Card Borders and Headers
    for (idx, card) in CARDS.iter().enumerate() {
        if in_rounded_rect(x, y, card.x0, card.y0, card.x1, card.y1, 10) {
            if on_rounded_rect_border(x, y, card.x0, card.y0, card.x1, card.y1, 10) {
                return EDGE;
            }

            let icon_hit = match idx {
                0 => icon_star(x, y, card.x0 + 24, card.y0 + 20),
                1 => icon_os(x, y, card.x0 + 24, card.y0 + 20),
                2 => icon_speed(x, y, card.x0 + 24, card.y0 + 20),
                3 => icon_verified(x, y, card.x0 + 24, card.y0 + 20),
                _ => None,
            };
            if let Some(c) = icon_hit {
                return c;
            }

            if font::text_at(x, y, card.x0 + 44, card.y0 + 12, 2, card.caption) {
                return card.caption_color;
            }
        }
    }

    // Card 1: identity of this binary
    if (170..372).contains(&y) && (30..690).contains(&x) {
        if font::text_at(x, y, 48, 196, 5, b"Hello World") {
            return TEXT;
        }
        let mut v = [0u8; 24];
        let n = write_lit(b"build ", &mut v);
        let dn = write_lit(BUILD_VERSION.as_bytes(), &mut v[n..]);
        if font::text_at(x, y, 48, 296, 2, &v[..n + dn]) {
            return ACCENT;
        }
        if font::text_at(x, y, 48, 326, 2, b"compiled into this static ELF") {
            return MUTED;
        }
        if font::text_at(x, y, 48, 240, 2, b"Ring 3 user-space isolated process") {
            return MUTED;
        }
        return CARD;
    }

    // Card 2: OS identity read back from the running image
    if (440..592).contains(&y) && (30..690).contains(&x) {
        let mut v = [0u8; 24];
        match &app.policy_version {
            Probe::Ok(p) => {
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

    // Card 3: what the launch path produced
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

    // Card 4: scope
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
                    let (ix, iy) = (input.x as usize, input.y as usize);
                    let tapped_back =
                        input.kind == 1 && ((24..100).contains(&ix) && (28..68).contains(&iy));
                    let tapped_home = input.kind == 1 && iy >= 1400;
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
