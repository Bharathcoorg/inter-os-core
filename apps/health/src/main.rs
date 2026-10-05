//! InterOS Sovereign Health & Biometric Hub
//!
//! Reads the sensor platform and plots only what the platform actually returns.
//!
//! The kernel sensor subsystem (`kernel::sensor::SensorManager`) exposes exactly
//! three devices: an IMU accelerometer, a proximity sensor and an ambient light
//! sensor. There is no heart-rate, blood-oxygen or pedometer backend anywhere in
//! it, and no paired medical device is bound to this platform. Those metrics are
//! therefore listed as unavailable with that reason, and no trace, average or
//! count is ever drawn for them.
//!
//! The chart plots real `lux` samples returned by `sensor_read`. When that
//! syscall reports no sensor hardware, the chart stays an empty grid with the
//! sample count reading zero. It is never filled with a plausible-looking
//! waveform.

#![no_std]
#![no_main]

mod font;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Service id of the sensor service in the kernel service registry.
const SENSOR_SERVICE_ID: u8 = 11;

/// Rolling window of real samples kept for the trace.
const MAX_SAMPLES: usize = 96;

/// Sample interval in 1 kHz boot ticks.
const SAMPLE_PERIOD_TICKS: u64 = 500;

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// What `sensor_read` said.
enum Telemetry {
    /// The call succeeded and the payload declared hardware.
    Present {
        lux: i64,
        accel: i64,
        near: bool,
        orientation: Orientation,
    },
    /// The call succeeded and the payload declared that no sensor is bound.
    Absent { status: [u8; 16], status_len: u8, error: [u8; 64], error_len: u8 },
    /// The call was refused by the capability check.
    Denied(i64),
    /// The call succeeded but the payload did not carry the fields we need.
    Malformed,
}

/// Screen orientation word, as the kernel spells it in `determine_orientation`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Orientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
    FaceUp,
    FaceDown,
    Other,
}

impl Orientation {
    fn from_kernel(s: &[u8]) -> Self {
        match s {
            b"Portrait" => Self::Portrait,
            b"PortraitUpsideDown" => Self::PortraitUpsideDown,
            b"LandscapeLeft" => Self::LandscapeLeft,
            b"LandscapeRight" => Self::LandscapeRight,
            b"FaceUp" => Self::FaceUp,
            b"FaceDown" => Self::FaceDown,
            _ => Self::Other,
        }
    }

    fn word(&self) -> &'static [u8] {
        match self {
            Self::Portrait => b"portrait",
            Self::PortraitUpsideDown => b"portrait upside down",
            Self::LandscapeLeft => b"landscape left",
            Self::LandscapeRight => b"landscape right",
            Self::FaceUp => b"face up",
            Self::FaceDown => b"face down",
            Self::Other => b"unrecognised",
        }
    }
}

struct Health {
    telemetry: Telemetry,
    sensor_service: Option<(u8, u32)>,
    /// The kernel's own settings-inventory verdict for the "sensors" category.
    inventory: Option<i64>,
    /// Real `lux` samples, oldest first. Empty until the platform supplies some.
    samples: [i64; MAX_SAMPLES],
    sample_count: usize,
    last_tick: u64,
    status: [u8; 64],
    status_len: usize,
    status_color: u32,
}

// -----------------------------------------------------------------------------
// Minimal flat-JSON reader
// -----------------------------------------------------------------------------

/// Finds the byte offset just past `"key":` in a flat JSON object.
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

fn json_bool(buf: &[u8], key: &[u8]) -> Option<bool> {
    let i = json_value(buf, key)?;
    if i >= buf.len() {
        return None;
    }
    match buf[i] {
        b't' if buf[i..].starts_with(b"true") => Some(true),
        b'f' if buf[i..].starts_with(b"false") => Some(false),
        _ => None,
    }
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

/// Copies a JSON string value into `out`, returning its length.
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

// -----------------------------------------------------------------------------
// Real sources
// -----------------------------------------------------------------------------

/// One `sensor_read` call, decoded. Nothing is defaulted: every field that the
/// kernel did not send leaves the telemetry in a state that says so.
fn read_sensors() -> Telemetry {
    let mut buf = [0u8; 256];
    let n = match std::sensor_read(&mut buf) {
        Ok(n) => n,
        Err(e) => return Telemetry::Denied(e),
    };
    let payload = &buf[..n.min(buf.len())];

    match json_bool(payload, b"present") {
        Some(true) => {
            let lux = match json_i64(payload, b"lux") {
                Some(v) => v,
                None => return Telemetry::Malformed,
            };
            let accel = match json_i64(payload, b"accel") {
                Some(v) => v,
                None => return Telemetry::Malformed,
            };
            let near = match json_bool(payload, b"near") {
                Some(v) => v,
                None => return Telemetry::Malformed,
            };
            let mut word = [0u8; 32];
            let orientation = match json_str(payload, b"orientation", &mut word) {
                Some(k) => Orientation::from_kernel(&word[..k]),
                None => return Telemetry::Malformed,
            };
            Telemetry::Present { lux, accel, near, orientation }
        }
        Some(false) => {
            let mut status = [0u8; 16];
            let mut error = [0u8; 64];
            let status_len = json_str(payload, b"status", &mut status).unwrap_or(0) as u8;
            let error_len = json_str(payload, b"error", &mut error).unwrap_or(0) as u8;
            Telemetry::Absent { status, status_len, error, error_len }
        }
        None => Telemetry::Malformed,
    }
}

/// Live lifecycle record of the sensor service. `service_query` needs no
/// capability, so this is a genuine read of kernel state.
fn read_sensor_service() -> Option<(u8, u32)> {
    let mut out = [0u8; 12];
    match std::service_query(SENSOR_SERVICE_ID, &mut out) {
        Ok(n) if n >= 12 => Some((
            out[1],
            u32::from_le_bytes([out[6], out[7], out[8], out[9]]),
        )),
        _ => None,
    }
}

/// The kernel's settings-inventory verdict for the `sensors` category.
fn read_inventory() -> Option<i64> {
    std::settings_hardware_status("sensors").ok()
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
    let mut n = 0;
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
const CORAL: u32 = 0xFFFF5E7E;

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

/// Vector ECG Pulse & Heart icon for Health Hub header.
fn icon_pulse(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq <= 17 * 17 {
        // ECG waveform line
        let on_ecg = match dx {
            -11..=-7 => dy == 0,
            -6 => dy == -2 || dy == -1,
            -5 => dy == -4 || dy == -3,
            -4 => dy == -1 || dy == 0,
            -3 => dy == 2 || dy == 3,
            -2 => dy == -6 || dy == -5,
            -1 => dy == -9 || dy == -8,
            0 => dy == -3 || dy == -2,
            1 => dy == 4 || dy == 5,
            2 => dy == 8 || dy == 9,
            3 => dy == 4 || dy == 3,
            4 => dy == -1 || dy == 0,
            5..=11 => dy == 0,
            _ => false,
        };
        if on_ecg {
            return Some(0xFFFFFFFF);
        }
        if dist_sq >= 15 * 15 {
            return Some(CORAL);
        }
        return Some(0xFF2A101C);
    }
    None
}

/// Vector Microchip icon for Sensor Bus card.
fn icon_chip(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dx.abs() <= 8 && dy.abs() <= 8 {
        if dx.abs() == 8 || dy.abs() == 8 {
            return Some(ACCENT);
        }
        if dx.abs() <= 5 && dy.abs() <= 5 && (dx.abs() == 5 || dy.abs() == 5) {
            return Some(MUTED);
        }
        return Some(CARD_RAISED);
    }
    if (dy.abs() == 2 || dy.abs() == 5) && (dx.abs() >= 9 && dx.abs() <= 12) {
        return Some(ACCENT);
    }
    if (dx.abs() == 2 || dx.abs() == 5) && (dy.abs() >= 9 && dy.abs() <= 12) {
        return Some(ACCENT);
    }
    None
}

/// Vector Activity Rings / Vitals icon for Metrics card.
fn icon_vitals(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if (dist_sq >= 9 * 9 && dist_sq <= 12 * 12) && dy <= 6 {
        return Some(OK);
    }
    if (dist_sq >= 5 * 5 && dist_sq <= 7 * 7) && (dx >= -5 && dy >= -5) {
        return Some(ACCENT);
    }
    if dist_sq <= 2 * 2 {
        return Some(0xFFFFFFFF);
    }
    None
}

/// Vector Photodiode / Sun icon for Ambient Light Monitor card.
fn icon_sun(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq <= 5 * 5 {
        return Some(WARN);
    }
    let is_ray = (dx.abs() <= 1 && (dy.abs() >= 7 && dy.abs() <= 11))
        || (dy.abs() <= 1 && (dx.abs() >= 7 && dx.abs() <= 11))
        || ((dx - dy).abs() <= 1 && (dx.abs() >= 5 && dx.abs() <= 8))
        || ((dx + dy).abs() <= 1 && (dx.abs() >= 5 && dx.abs() <= 8));
    if is_ray {
        return Some(WARN);
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

const CARDS: [Card; 3] = [
    Card { x0: 30, y0: 108, x1: 690, y1: 456, caption: b"SENSOR BUS TELEMETRY" },
    Card { x0: 30, y0: 472, x1: 690, y1: 936, caption: b"BIOMETRIC & SENSOR METRICS" },
    Card { x0: 30, y0: 952, x1: 690, y1: 1312, caption: b"AMBIENT LIGHT TRACE (REAL LUX)" },
];

/// One row of the metric table.
struct Metric {
    label: &'static [u8],
    verdict: &'static [u8],
    reason: &'static [u8],
    color: u32,
}

const STATIC_METRICS_HEAD: [Metric; 3] = [
    Metric {
        label: b"Heart rate",
        verdict: b"unavailable",
        reason: b"no PPG/HR backend in the sensor manager",
        color: WARN,
    },
    Metric {
        label: b"Blood oxygen",
        verdict: b"unavailable",
        reason: b"no SpO2 backend in the sensor manager",
        color: WARN,
    },
    Metric {
        label: b"Steps",
        verdict: b"unavailable",
        reason: b"no pedometer backend on this platform",
        color: WARN,
    },
];

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

#[inline(never)]
fn field(x: usize, y: usize, top: usize, left: &[u8], right: &[u8], right_color: u32) -> Option<u32> {
    let right_x = 676usize.saturating_sub(font::text_width(right, 2));
    if font::text_at(x, y, 44, top, 2, left) {
        return Some(MUTED);
    }
    if font::text_at(x, y, right_x, top, 2, right) {
        return Some(right_color);
    }
    None
}

#[inline(never)]
fn line(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    if font::text_at(x, y, 44, top, 2, text) {
        return Some(color);
    }
    None
}

#[inline(never)]
fn right_line(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    let x0 = 676usize.saturating_sub(font::text_width(text, 2));
    if font::text_at(x, y, x0, top, 2, text) {
        return Some(color);
    }
    None
}

fn render(surface: u32, app: &Health) {
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
fn pixel(x: usize, y: usize, app: &Health) -> u32 {
    // Top Bar (y: 0..96)
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }

        // Back Button
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let on_border = on_rounded_rect_border(x, y, 24, 28, 100, 68, 8);
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            return if on_border || chev { TEXT } else { CARD_RAISED };
        }

        // Pulse Icon
        if let Some(c) = icon_pulse(x, y, 134, 48) {
            return c;
        }

        // Title and Subtitle
        if font::text_at(x, y, 164, 32, 3, b"Health Hub") {
            return TEXT;
        }
        if font::text_at(x, y, 166, 62, 1, b"SOVEREIGN BIOMETRIC PLATFORM") {
            return FAINT;
        }

        // Live status pill in top header (x: 550..690, y: 34..62)
        if in_rounded_rect(x, y, 540, 34, 690, 62, 6) {
            if on_rounded_rect_border(x, y, 540, 34, 690, 62, 6) {
                return EDGE;
            }
            let is_present = matches!(app.telemetry, Telemetry::Present { .. });
            let dot_color = if is_present { OK } else { WARN };
            let (dx, dy) = (x as isize - 556, y as isize - 48);
            if dx * dx + dy * dy <= 4 * 4 {
                return dot_color;
            }
            let status_lbl: &[u8] = if is_present { b"ONLINE" } else { b"STANDBY" };
            if font::text_at(x, y, 568, 41, 2, status_lbl) {
                return dot_color;
            }
            return CARD_RAISED;
        }

        return CARD;
    }

    // Bottom Status Pill Card (y: 1324..1380)
    if (1324..1380).contains(&y) && (30..690).contains(&x) {
        if on_rounded_rect_border(x, y, 30, 1324, 690, 1380, 8) {
            return EDGE;
        }
        if in_rounded_rect(x, y, 30, 1324, 690, 1380, 8) {
            let (dx, dy) = (x as isize - 48, y as isize - 1352);
            if dx * dx + dy * dy <= 4 * 4 {
                return app.status_color;
            }
            if font::text_at(x, y, 62, 1344, 2, &app.status[..app.status_len]) {
                return app.status_color;
            }
            return CARD_INNER;
        }
    }

    // Card Borders and Headers
    for (idx, card) in CARDS.iter().enumerate() {
        if in_rounded_rect(x, y, card.x0, card.y0, card.x1, card.y1, 10) {
            if on_rounded_rect_border(x, y, card.x0, card.y0, card.x1, card.y1, 10) {
                return EDGE;
            }

            // Card Header Icons
            let icon_hit = match idx {
                0 => icon_chip(x, y, card.x0 + 24, card.y0 + 20),
                1 => icon_vitals(x, y, card.x0 + 24, card.y0 + 20),
                2 => icon_sun(x, y, card.x0 + 24, card.y0 + 20),
                _ => None,
            };
            if let Some(c) = icon_hit {
                return c;
            }

            if font::text_at(x, y, card.x0 + 44, card.y0 + 12, 2, card.caption) {
                return ACCENT;
            }
        }
    }

    // Card Contents
    if (108..456).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = sensor_bus_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (472..936).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = metrics_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (952..1312).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = trace_card(x, y, app) {
            return c;
        }
        return CARD;
    }

    // Home Gesture Bar
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }

    BG
}

#[inline(never)]
fn sensor_bus_card(x: usize, y: usize, app: &Health) -> Option<u32> {
    let (verdict, color) = match &app.telemetry {
        Telemetry::Present { .. } => (b"reading".as_slice(), OK),
        Telemetry::Absent { .. } => (b"no sensor bound".as_slice(), WARN),
        Telemetry::Denied(_) => (b"refused".as_slice(), WARN),
        Telemetry::Malformed => (b"no data".as_slice(), WARN),
    };
    let mut word = [0u8; 32];
    let mut wn = write_lit(verdict, &mut word);
    if let Telemetry::Denied(e) = &app.telemetry {
        wn += write_lit(b" (", &mut word[wn..]);
        wn += fmt_i64(*e, &mut word[wn..]);
        if wn < word.len() {
            word[wn] = b')';
            wn += 1;
        }
    }
    if let Some(c) = field(x, y, 154, b"Sensor read", &word[..wn], color) {
        return Some(c);
    }
    if line(x, y, 174, b"sensor_read() -> kernel sensor manager", FAINT).is_some() {
        return Some(FAINT);
    }

    // Quoting kernel error verbatim if absent
    if let Telemetry::Absent { status, status_len, error, error_len } = &app.telemetry {
        if font::text_at(x, y, 44, 208, 2, b"Kernel reports:") {
            return Some(MUTED);
        }
        if *status_len > 0
            && font::text_at(x, y, 260, 208, 2, &status[..*status_len as usize])
        {
            return Some(WARN);
        }
        let err = &error[..*error_len as usize];
        let split = err.len().min(39);
        if font::text_at(x, y, 44, 238, 2, &err[..split]) {
            return Some(TEXT);
        }
        if font::text_at(x, y, 44, 262, 2, &err[split..]) {
            return Some(TEXT);
        }
    }

    // Live sensor service record
    let mut word = [0u8; 32];
    let (svc_color, wn) = match app.sensor_service {
        Some((2, gen)) => {
            let n = write_lit(service_state_word(2), &mut word);
            let n = n + write_lit(b" gen ", &mut word[n..]);
            (OK, n + fmt_u64(gen as u64, &mut word[n..]))
        }
        Some((state, gen)) => {
            let n = write_lit(service_state_word(state), &mut word);
            let n = n + write_lit(b" gen ", &mut word[n..]);
            (WARN, n + fmt_u64(gen as u64, &mut word[n..]))
        }
        None => (WARN, write_lit(b"no record", &mut word)),
    };
    if let Some(c) = field(x, y, 310, b"Sensor service", &word[..wn], svc_color) {
        return Some(c);
    }
    if line(x, y, 330, b"service_query(11)", FAINT).is_some() {
        return Some(FAINT);
    }

    // Settings inventory
    let mut word = [0u8; 32];
    let (inv_color, wn) = match app.inventory {
        Some(code) => (FAINT, write_lit(inventory_word(code), &mut word)),
        None => (WARN, write_lit(b"refused", &mut word)),
    };
    if let Some(c) = field(x, y, 366, b"Settings inventory", &word[..wn], inv_color) {
        return Some(c);
    }
    if line(x, y, 386, b"settings_hardware_status(\"sensors\")", FAINT).is_some() {
        return Some(FAINT);
    }

    line(x, y, 420, b"Nothing is invented to fill the chart below.", MUTED)
}

#[inline(never)]
fn metrics_card(x: usize, y: usize, app: &Health) -> Option<u32> {
    for (i, metric) in STATIC_METRICS_HEAD.iter().enumerate() {
        let top = 518 + i * 56;
        if let Some(c) = field(x, y, top, metric.label, metric.verdict, metric.color) {
            return Some(c);
        }
        if line(x, y, top + 20, metric.reason, FAINT).is_some() {
            return Some(FAINT);
        }
    }

    let base = 518 + STATIC_METRICS_HEAD.len() * 56;
    match &app.telemetry {
        Telemetry::Present { lux, accel, near, orientation } => {
            let mut word = [0u8; 32];
            let n = write_lit(b"measured: ", &mut word);
            let n = n + fmt_i64(*lux, &mut word[n..]);
            let n = n + write_lit(b" lx", &mut word[n..]);
            if let Some(c) = field(x, y, base, b"Ambient light", &word[..n], OK) {
                return Some(c);
            }
            if line(x, y, base + 20, b"sensor_read() -> lux", FAINT).is_some() {
                return Some(FAINT);
            }

            let mut word = [0u8; 32];
            let n = write_lit(b"measured: ", &mut word);
            let n = n + fmt_i64(*accel, &mut word[n..]);
            let n = n + write_lit(b" m/s2", &mut word[n..]);
            if let Some(c) = field(x, y, base + 56, b"Accelerometer Y", &word[..n], OK) {
                return Some(c);
            }
            if line(x, y, base + 76, b"sensor_read() -> accel", FAINT).is_some() {
                return Some(FAINT);
            }

            let near_word: &[u8] = if *near { b"near" } else { b"far" };
            if let Some(c) = field(x, y, base + 112, b"Proximity", near_word, OK) {
                return Some(c);
            }
            if line(x, y, base + 132, b"sensor_read() -> near", FAINT).is_some() {
                return Some(FAINT);
            }

            if let Some(c) = field(x, y, base + 168, b"Orientation", orientation.word(), OK) {
                return Some(c);
            }
            if line(x, y, base + 188, b"sensor_read() -> orientation", FAINT).is_some() {
                return Some(FAINT);
            }
        }
        _ => {
            for (i, label) in [b"Ambient light".as_slice(), b"Accelerometer"].iter().enumerate() {
                let top = base + i * 56;
                if let Some(c) = field(x, y, top, label, b"no reading", WARN) {
                    return Some(c);
                }
                if line(x, y, top + 20, b"the sensor bus returned no data", FAINT).is_some() {
                    return Some(FAINT);
                }
            }
        }
    }

    line(x, y, 904, b"Every measured value came from one sensor_read call.", MUTED)
}

#[inline(never)]
fn trace_card(x: usize, y: usize, app: &Health) -> Option<u32> {
    const PX0: usize = 56;
    const PX1: usize = 664;
    const PY0: usize = 998;
    const PY1: usize = 1240;

    if x >= PX0 && x < PX1 && y >= PY0 && y < PY1 {
        let gx = (x - PX0) % 120;
        let gy = (y - PY0) % 48;
        if x == PX0 || x == PX1 - 1 || y == PY0 || y == PY1 - 1 {
            return Some(EDGE);
        }
        if gx == 0 || gy == 0 {
            return Some(CARD_RAISED);
        }
        if app.sample_count > 0 {
            // Check if pixel is part of waveform or area glow
            let mut lo = app.samples[0];
            let mut hi = app.samples[0];
            for i in 1..app.sample_count {
                let v = app.samples[i];
                if v < lo { lo = v; }
                if v > hi { hi = v; }
            }
            let span = hi - lo;

            // Plot sample lines
            for i in 1..app.sample_count {
                let y0 = plot_y(app.samples[i - 1], lo, span, PY0 + 10, PY1 - 10);
                let y1 = plot_y(app.samples[i], lo, span, PY0 + 10, PY1 - 10);
                let cx0 = PX0 + (PX1 - 1 - PX0) * (i - 1) / app.sample_count.max(1);
                let cx1 = PX0 + (PX1 - 1 - PX0) * i / app.sample_count.max(1);

                if x >= cx0 && x <= cx1 {
                    let y_min = y0.min(y1);
                    let y_max = y0.max(y1);
                    if y >= y_min && y <= y_max + 1 {
                        return Some(ACCENT);
                    }
                    if y > y_max && (y - y_max) % 4 == 0 {
                        return Some(0x22173A5E);
                    }
                }
            }
        }
        return Some(CARD_INNER);
    }

    if app.sample_count == 0 {
        if font::text_at(x, y, font::text_center(PX0, PX1, 2, b"0 samples recorded."), 1090, 2, b"0 samples recorded.") {
            return Some(TEXT);
        }
        if font::text_at(
            x,
            y,
            font::text_center(PX0, PX1, 2, b"Nothing to plot, and none is invented."),
            1122,
            2,
            b"Nothing to plot, and none is invented.",
        ) {
            return Some(MUTED);
        }
        if font::text_at(
            x,
            y,
            font::text_center(PX0, PX1, 2, b"sensor_read() reported no sensor hardware."),
            1154,
            2,
            b"sensor_read() reported no sensor hardware.",
        ) {
            return Some(FAINT);
        }
        return None;
    }

    // Real samples: normalise against observed extremes
    let mut lo = app.samples[0];
    let mut hi = app.samples[0];
    for i in 1..app.sample_count {
        let v = app.samples[i];
        if v < lo { lo = v; }
        if v > hi { hi = v; }
    }

    // Axis annotations
    let mut word = [0u8; 24];
    let mut n = write_lit(b"max ", &mut word);
    let mut n2 = fmt_i64(hi, &mut word[n..]);
    n += n2;
    n2 = write_lit(b" lx", &mut word[n..]);
    n += n2;
    if right_line(x, y, PY1 + 10, &word[..n], OK).is_some() {
        return Some(OK);
    }
    let mut word = [0u8; 24];
    let n = write_lit(b"min ", &mut word);
    let n = n + fmt_i64(lo, &mut word[n..]);
    let n = n + write_lit(b" lx", &mut word[n..]);
    if line(x, y, PY1 + 10, &word[..n], MUTED).is_some() {
        return Some(MUTED);
    }

    let mut word = [0u8; 24];
    let n = write_lit(b"samples: ", &mut word);
    let n = n + fmt_u64(app.sample_count as u64, &mut word[n..]);
    if right_line(x, y, 1276, &word[..n], MUTED).is_some() {
        return Some(MUTED);
    }
    if line(x, y, 1276, b"real sensor_read() lux", FAINT).is_some() {
        return Some(FAINT);
    }

    None
}

/// Maps a real sample onto the plot, using only the window's own extremes.
fn plot_y(v: i64, lo: i64, span: i64, py0: usize, py1: usize) -> usize {
    let height = (py1 - 1 - py0) as i64;
    if span <= 0 {
        return py0;
    }
    let up = ((v - lo) * height) / span;
    let y = (py1 - 1) as i64 - up;
    y.clamp(py0 as i64, (py1 - 1) as i64) as usize
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

fn status(app: &mut Health, text: &[u8], color: u32) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
    app.status_color = color;
}

fn sample(app: &mut Health) {
    app.telemetry = read_sensors();
    if let Telemetry::Present { lux, .. } = &app.telemetry {
        if app.sample_count == MAX_SAMPLES {
            app.samples.copy_within(1..MAX_SAMPLES, 0);
            app.sample_count = MAX_SAMPLES - 1;
        }
        app.samples[app.sample_count] = *lux;
        app.sample_count += 1;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[HEALTH (Ring 3)] Sovereign Health & Biometric Hub starting.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[HEALTH] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut app = Health {
        telemetry: Telemetry::Malformed,
        sensor_service: None,
        inventory: None,
        samples: [0; MAX_SAMPLES],
        sample_count: 0,
        last_tick: 0,
        status: [0; 64],
        status_len: 0,
        status_color: MUTED,
    };

    sample(&mut app);
    app.sensor_service = read_sensor_service();
    app.inventory = read_inventory();

    match &app.telemetry {
        Telemetry::Present { .. } => {
            std::log(0, "[HEALTH] sensor_read() returned live sensor telemetry.").ok();
        }
        Telemetry::Absent { error_len, .. } => {
            std::log(
                0,
                "[HEALTH] sensor_read() reports no sensor hardware is bound; showing the empty state.",
            )
            .ok();
            let _ = error_len;
        }
        Telemetry::Denied(e) => {
            std::log(
                0,
                "[HEALTH] sensor_read() refused: this app holds no sensor control capability.",
            )
            .ok();
            let _ = e;
        }
        Telemetry::Malformed => {
            std::log(0, "[HEALTH] sensor_read() returned a payload with no telemetry fields.").ok();
        }
    }

    let status_text: &[u8] = match &app.telemetry {
        Telemetry::Present { .. } => b"live sensor telemetry; chart shows real samples",
        Telemetry::Absent { .. } => b"no sensor hardware bound; chart is empty on purpose",
        Telemetry::Denied(_) => b"sensor read refused; chart is empty on purpose",
        Telemetry::Malformed => b"sensor payload had no telemetry fields",
    };
    status(&mut app, status_text, ACCENT);

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[HEALTH] Initial frame presented to hardware scanout.").ok();

    let mut refresh = true;
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
                        std::log(0, "[HEALTH] Navigation gesture. Exiting to launcher.").ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        let tapped_back =
                            (24..100).contains(&input.x) && (28..68).contains(&input.y);
                        if tapped_back || input.y >= 1400 {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }
                        sample(&mut app);
                        app.sensor_service = read_sensor_service();
                        app.inventory = read_inventory();
                        status(&mut app, b"re-read sensor bus, service record and inventory", ACCENT);
                        refresh = true;
                    }
                }
            }
        }

        if let Ok(now) = std::get_ticks(0) {
            if now.saturating_sub(app.last_tick) >= SAMPLE_PERIOD_TICKS {
                app.last_tick = now;
                sample(&mut app);
                refresh = true;
            }
        }
        if refresh {
            refresh = false;
            render(surface, &app);
            let _ = std::surface_flip(6);
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Health app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
