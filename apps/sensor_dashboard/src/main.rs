//! InterOS Core Open-Source Reference App: Sensor Dashboard
//!
//! A hardware sensor INVENTORY. It tells the user exactly what their device can
//! measure, and for every channel it either draws a real reading or names the
//! reason there is none.
//!
//! WHY AN INVENTORY AND NOT A CHART: the platform binds no sensors at all. The
//! kernel's own verdicts are absent, and they are absent by construction, not
//! by accident:
//!
//! - `kernel::sensor::SensorManager::is_hardware_present()` returns a hardcoded
//!   `false` (kernel/src/sensor/mod.rs:32), so `sys_sensor_read` serialises
//!   `{"present":false,...}` for the accelerometer, gyroscope, ambient light and
//!   proximity channels. There is no code path that could make it true.
//! - `kernel::resource::thermal::ThermalManager::sensor_present` starts `false`
//!   and is set only by `update_temperature`, which no boot path calls: the
//!   temperatures the governor self-test uses are applied to a *local* instance
//!   and the global is asserted to still report nothing. So `sys_thermal_query`
//!   returns `DeviceError` rather than five zone temperatures.
//! - `sys_battery_query` reports `{"present":false}` because the fuel gauge's
//!   `hardware_present` is likewise only set by a self-test on a local object.
//!
//! So there is no trace to plot. Drawing a waveform here would be inventing the
//! data, which is the one thing this app must never do. Instead each row of the
//! inventory carries a verdict that came from a real syscall and the reason that
//! verdict has that value.
//!
//! Every value on screen is one of:
//!   - a decoded field of a real `sensor_read` / `thermal_query` /
//!     `battery_query` / `resource_query` payload,
//!   - the kernel's own `status` / `error` string, quoted verbatim,
//!   - the negative `SyscallResult` the kernel returned, named,
//!   - a `service_query` / `settings_hardware_status` verdict,
//!   - or the app's own build-time constants.
//!
//! There is no code path in this file that produces a measurement. If a sensor
//! were bound, the same code would plot it: `read_sensors` returns the live
//! values and the chart branch is driven entirely by that decode.
//!
//! Banded rendering (`render_band`): a tap re-reads the bus, the inventory and
//! the power regulator, and only the rows those re-reads can move are
//! re-rasterised, instead of all 1,036,800 pixels on every event.

#![no_std]
#![no_main]

mod font;
mod outline_text;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Service id of the sensor service in the kernel service registry.
const SENSOR_SERVICE_ID: u8 = 11;

/// The version string compiled into this binary from the crate manifest.
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// Result of one real probe. Nothing here is ever defaulted to a made-up value.
enum Probe<T: Copy> {
    Ok(T),
    /// The syscall returned a negative kernel status code.
    Refused(i64),
}

/// One `sensor_read` call, decoded.
enum Telemetry {
    /// The payload declared hardware and carried the fields we need.
    Present { lux: i64, accel: i64, near: bool, orientation: [u8; 24], orientation_len: usize },
    /// The payload declared that no sensor is bound.
    Absent { status: [u8; 24], status_len: usize, error: [u8; 96], error_len: usize },
    /// The call was refused by the capability check.
    Denied(i64),
    /// The call succeeded but the payload had no `present` field.
    Malformed,
}

/// One `thermal_query` call, decoded.
enum Thermal {
    /// Zones were serialised. `present` is the kernel's own flag.
    Zones { present: bool, cpu: i64, gpu: i64, battery: i64, modem: i64, charging: i64, state: [u8; 16], state_len: usize, throttling: bool },
    /// `sys_thermal_query` returns `DeviceError` (-12) when no sensor is bound.
    Refused(i64),
    Malformed,
}

/// One `battery_query` call, decoded.
enum Battery {
    Reading { voltage_mv: i64, soc: i64, capacity: i64, charging: [u8; 20], charging_len: usize, temp: i64 },
    Absent { governor: [u8; 20], governor_len: usize, mhz: i64 },
    Denied(i64),
    Malformed,
}

struct Dashboard {
    task_id: u64,
    ticks: Probe<u64>,
    surface_id: u32,
    telemetry: Telemetry,
    thermal: Thermal,
    battery: Battery,
    sensor_service: Option<(u8, u32)>,
    inv_display: Option<i64>,
    inv_sensors: Option<i64>,
    inv_battery: Option<i64>,
    inv_wifi: Option<i64>,
    inv_cellular: Option<i64>,
    status: [u8; 64],
    status_len: usize,
    status_color: u32,
}

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
            let mut orientation = [0u8; 24];
            let orientation_len = match json_str(payload, b"orientation", &mut orientation) {
                Some(k) => k,
                None => return Telemetry::Malformed,
            };
            Telemetry::Present { lux, accel, near, orientation, orientation_len }
        }
        Some(false) => {
            let mut status = [0u8; 24];
            let mut error = [0u8; 96];
            let status_len = json_str(payload, b"status", &mut status).unwrap_or(0);
            let error_len = json_str(payload, b"error", &mut error).unwrap_or(0);
            Telemetry::Absent { status, status_len, error, error_len }
        }
        None => Telemetry::Malformed,
    }
}

fn read_thermal() -> Thermal {
    let mut buf = [0u8; 256];
    let n = match std::thermal_query(&mut buf) {
        Ok(n) => n,
        Err(e) => return Thermal::Refused(e),
    };
    let payload = &buf[..n.min(buf.len())];

    let present = match json_bool(payload, b"present") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let cpu = match json_i64(payload, b"cpu") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let gpu = match json_i64(payload, b"gpu") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let battery = match json_i64(payload, b"battery") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let modem = match json_i64(payload, b"modem") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let charging = match json_i64(payload, b"charging") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let mut state = [0u8; 16];
    let state_len = match json_str(payload, b"state", &mut state) {
        Some(k) => k,
        None => return Thermal::Malformed,
    };
    let throttling = match json_bool(payload, b"throttling") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    Thermal::Zones { present, cpu, gpu, battery, modem, charging, state, state_len, throttling }
}

fn read_battery() -> Battery {
    let mut buf = [0u8; 256];
    let n = match std::battery_query(&mut buf) {
        Ok(n) => n,
        Err(e) => return Battery::Denied(e),
    };
    let payload = &buf[..n.min(buf.len())];

    match json_bool(payload, b"present") {
        Some(true) => {
            let voltage_mv = match json_i64(payload, b"voltage_mv") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let soc = match json_i64(payload, b"soc") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let capacity = match json_i64(payload, b"capacity") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let temp = match json_i64(payload, b"temp") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let mut charging = [0u8; 20];
            let charging_len = match json_str(payload, b"charging", &mut charging) {
                Some(k) => k,
                None => return Battery::Malformed,
            };
            Battery::Reading { voltage_mv, soc, capacity, charging, charging_len, temp }
        }
        Some(false) => {
            let mut governor = [0u8; 20];
            let governor_len = json_str(payload, b"governor", &mut governor).unwrap_or(0);
            let mhz = json_i64(payload, b"mhz").unwrap_or(0);
            Battery::Absent { governor, governor_len, mhz }
        }
        None => Battery::Malformed,
    }
}

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

fn read_inventory(category: &str) -> Option<i64> {
    std::settings_hardware_status(category).ok()
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
const PURPLE: u32 = 0xFFA78BFA;

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

/// Vector Telemetry Radar / Gyro icon for header.
fn icon_radar(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq <= 16 * 16 {
        // Outer ring
        if dist_sq >= 14 * 14 {
            return Some(ACCENT);
        }
        // Middle ring
        if dist_sq >= 8 * 8 && dist_sq <= 10 * 10 {
            return Some(MUTED);
        }
        // Center blip
        if dist_sq <= 3 * 3 {
            return Some(OK);
        }
        // Radar sweep line (top-right quadrant)
        if dx >= 0 && dy <= 0 && (dx + dy).abs() <= 1 {
            return Some(ACCENT);
        }
        return Some(CARD_INNER);
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

/// Vector Channel inventory sliders icon.
fn icon_channels(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // 3 horizontal tracks with knobs
    for &(track_y, knob_x) in &[(-6, -3), (0, 4), (6, -1)] {
        if (dy - track_y).abs() == 0 && dx.abs() <= 10 {
            return Some(MUTED);
        }
        if (dx - knob_x).abs() <= 2 && (dy - track_y).abs() <= 3 {
            return Some(PURPLE);
        }
    }
    None
}

/// Vector Battery & Thermometer power icon.
fn icon_power(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Battery body
    if dx.abs() <= 8 && dy.abs() <= 5 {
        if dx.abs() == 8 || dy.abs() == 5 {
            return Some(WARN);
        }
        // Lightning bolt inside
        if (dx == -1 && dy <= 1 && dy >= -3)
            || (dy == 0 && dx >= -2 && dx <= 2)
            || (dx == 1 && dy >= 0 && dy <= 3)
        {
            return Some(TEXT);
        }
        return Some(CARD_INNER);
    }
    // Battery tip
    if dx == 9 && dy.abs() <= 2 {
        return Some(WARN);
    }
    None
}

/// Vector Terminal / Runtime icon.
fn icon_runtime(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // Terminal window frame
    if dx.abs() <= 10 && dy.abs() <= 8 {
        if dx.abs() == 10 || dy.abs() == 8 {
            return Some(EDGE);
        }
        // Top prompt >_
        if (dy == -3 && dx == -6)
            || (dy == -2 && dx == -5)
            || (dy == -1 && dx == -4)
            || (dy == 0 && dx == -5)
            || (dy == 1 && dx == -6)
        {
            return Some(ACCENT);
        }
        if dy == 1 && dx >= -1 && dx <= 3 {
            return Some(TEXT);
        }
        return Some(CARD_INNER);
    }
    None
}

// -----------------------------------------------------------------------------
// Type grid
// -----------------------------------------------------------------------------
// WHY ONE LINE PER ROW: the shared face's line box is 24 source rows, so the old
// "label line + provenance line" row needs 24 + 24 = 48px of pitch. This app
// draws up to 24 rows in four cards (6 + 9 + 6 + 3), which is 1152px of rows
// plus four captions and the closing line: more than the 1262px of card space a
// 720x1440 phone has, before a single pixel of padding. So every row is ONE line
// box and the provenance sits in the middle column between the label and the
// verdict. The verdict is measured first, so the middle column is always exactly
// as wide as it can be and the source is fitted into it - never clipped, never
// overprinted, never run off the card.

/// Body type scale. The shared face's 17px cap height at scale 1 is the same
/// cap height the private 8x8 face produced at scale 2.
const SC: usize = 1;

/// Heading scale (card captions and the app title).
const HSC: usize = 2;

/// Row pitch: one 24px line box plus an 8px gutter.
///
/// WHY 32 and not 34: the base grid is 8px, so a 24px line box plus a 10px
/// gutter is two-and-a-tenth rows - the gaps between rows were the only spacing
/// on this screen that was not a multiple of the design system's base. Thirty-two
/// also gives the tallest cards back room: the inventory card's ten rows now end
/// 20px higher, and the power card's worst branch (three thermal rows, two
/// battery rows and the settings inventory row) ends at 1152 inside a card that
/// closes at 1192.
const ROW_PITCH: usize = 32;

/// Left edge of every card row and standalone line.
///
/// WHY 40 and not 44: the card's corner radius is 10, so a 44px inset kept 34px
/// of unused space between the rim and the rows. Ten is the smallest inset that
/// still clears the radius, it is on the 8-point grid, and those 4px are given
/// to the middle column, which is the column this screen is short of.
const ROW_X0: usize = 40;
/// Right edge of every card row and standalone line (the card's inner edge).
const ROW_X1: usize = 680;
/// Gutter between a row's label, provenance and verdict columns.
const COL_GAP: usize = 14;
/// Column ceiling for a row label (the widest one is "Screen orientation").
const LABEL_W: usize = 176;
/// Column ceiling for a row verdict (the widest one is the four-zone reading).
const VERDICT_W: usize = 288;
/// Column ceiling for a card caption. The widest is "THERMAL & POWER REGULATOR".
const CAPTION_W: usize = 618;

/// First row baseline box inside a card. WHY +70: the caption is a 48px line box
/// that starts 14px below the card edge, so 14 + 48 + 8 puts the first row clear
/// of it.
const CARD_TOP: usize = 70;

struct Card {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    caption: &'static [u8],
}

// Card heights are sized for the WORST branch each card can draw, not for the
// branch this platform happens to produce: 6 rows for the bus and power cards,
// 9 rows plus the closing line for the inventory, 3 for the runtime card.
// 142 + 294 + 16 + 430 + 16 + 294 + 16 + 192 = 1400, which still leaves the
// home gesture pill (y 1418) clear.
const CARDS: [Card; 4] = [
    Card { x0: 30, y0: 142, x1: 690, y1: 436, caption: b"SENSOR BUS ARCHITECTURE" },
    Card { x0: 30, y0: 452, x1: 690, y1: 882, caption: b"CHANNEL INVENTORY" },
    Card { x0: 30, y0: 898, x1: 690, y1: 1192, caption: b"THERMAL & POWER REGULATOR" },
    Card { x0: 30, y0: 1208, x1: 690, y1: 1400, caption: b"RUNTIME ENVIRONMENT" },
];

/// Top of the first row box inside card `idx`.
const fn card_rows_y(idx: usize) -> usize {
    CARDS[idx].y0 + CARD_TOP
}

// -----------------------------------------------------------------------------
// Screen bands - the single source of truth for both the drawing in `pixel`
// and the `Dirty` -> band mapping used by the input path below.
// -----------------------------------------------------------------------------

/// Header status pill: drawn at (540, 34, 690, 62) and read from
/// `app.telemetry`, so a re-read is the only thing that can move it.
const PILL_X0: usize = 540;
const PILL_Y0: usize = 34;
const PILL_X1: usize = 690;
const PILL_Y1: usize = 62;
const PILL_R: usize = 6;

/// Status band card: drawn at (30, 96, 690, 136) and read from `app.status`,
/// which every re-read rewrites.
///
/// WHY 40 rows and not 38: a 38-row card is two rows off the 8-point grid the
/// rest of this screen is laid out on, and its text could only be centred by an
/// odd 7px inset. Forty rows puts the card on the grid and lets the status line
/// sit 8 rows below the rim, centred.
///
/// WHY this card's height can move when the pill's cannot: the pill is the
/// re-probe hit target (`PILL_Y0..PILL_Y1` inside `handle_touch_point`), so its
/// geometry is part of the app's input contract; this card is drawn from state
/// only and no tap lands on it.
const STATUS_X0: usize = 30;
const STATUS_Y0: usize = 96;
const STATUS_X1: usize = 690;
const STATUS_Y1: usize = 136;
const STATUS_R: usize = 6;

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

fn render(surface: u32, app: &Dashboard) {
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

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// tap actually re-read (see `Dirty`), and every row outside it is correct
/// because nothing it draws from moved.
fn render_band(surface: u32, app: &Dashboard, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
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

/// Which rows of the surface an input actually changed.
///
/// WHY this exists: the old loop called `render` for every tap, re-rasterising
/// all 1,036,800 pixels through `pixel` even though the rows a re-read can move
/// live in one header pill, one status band and three fixed cards.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// A tap re-read the sensor bus, the channel inventory and the power
    /// regulator: the header status pill, the status band and cards 0..=2.
    ///
    /// WHY card 3 (RUNTIME ENVIRONMENT) is not in that list: its rows are drawn
    /// from state no re-read touches - the task id and the surface id are fixed
    /// at boot and the uptime probe runs once, before the first frame - so its
    /// rows cannot move and are left exactly as drawn. The home gesture pill
    /// and the page background are static for the same reason.
    Refresh,
    /// A change whose row extent is not tracked exactly: the whole surface.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several touches before the app is scheduled
/// again; rendering only the last region would leave the earlier one stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

/// Maps a `Dirty` onto the bands it re-rasterises.
fn repaint(surface: u32, app: &Dashboard, dirty: Dirty) {
    match dirty {
        Dirty::None => {}
        Dirty::Refresh => {
            // The status pill is the only header row a re-read can move.
            render_band(surface, app, PILL_Y0, PILL_Y1);
            // One contiguous band from the status card down to the bottom of
            // the last card. WHY contiguous rather than one write per card: the
            // gaps between the cards are background that is rewritten
            // identically, and four separate band writes would save 40 rows of
            // pure background at the cost of three extra surface write calls.
            //
            // WHY this reaches the runtime card and not just the status card:
            // that card draws an Uptime row from `get_ticks()`. The tick
            // counter is monotonic, so a refresh that leaves the row out of the
            // band would publish a fresh reading and keep showing the boot
            // value -- a number that moves only when the app is relaunched,
            // labelled as seconds since boot. Stopping the band short of it was
            // correct only because the tick probe ran once at boot; the row is
            // now re-read with everything else and the band covers it.
            render_band(surface, app, STATUS_Y0, CARDS[CARDS.len() - 1].y1);
        }
        Dirty::All => render(surface, app),
    }
}

#[inline(never)]
fn pixel(x: usize, y: usize, app: &Dashboard) -> u32 {
    // Header (y: 0..92)
    if y < 92 {
        if y >= 88 {
            return EDGE;
        }

        // Circular back button disc (cx: 62, cy: 48, radius: 22)
        let dx = x as isize - 62;
        let dy = y as isize - 48;
        let dsq = dx * dx + dy * dy;
        if dsq <= 22 * 22 {
            // Anti-aliased disc border rim
            if dsq >= 20 * 20 {
                return EDGE;
            }
            // Crisp vector chevron: apex at (58, 48), arms pointing up-right and down-right
            if dx <= 2 && dx >= -6 {
                let arm_dy = (dx - (-6)).abs();
                if (dy.abs() - arm_dy).abs() <= 1 {
                    return TEXT;
                }
            }
            return CARD_RAISED;
        }

        // Radar / Gyro Icon
        if let Some(c) = icon_radar(x, y, 134, 48) {
            return c;
        }

        // App Title and Subtitle.
        // WHY scale 2 and y0 12: a scale-3 run of this title is 423px wide and
        // would run into the status pill at x 540; the shared face's line box is
        // 24 rows, so the 48px title occupies 12..60 and the subtitle 62..86.
        if font::ink(x, y, 164, 12, HSC, b"Sensors & HAL", 376, TEXT, CARD) {
            return TEXT;
        }
        if font::ink(x, y, 164, 62, SC, b"DEVICE HARDWARE INVENTORY", 512, FAINT, CARD) {
            return FAINT;
        }

        // Status pill on top right
        if in_rounded_rect(x, y, PILL_X0, PILL_Y0, PILL_X1, PILL_Y1, PILL_R) {
            if on_rounded_rect_border(x, y, PILL_X0, PILL_Y0, PILL_X1, PILL_Y1, PILL_R) {
                return EDGE;
            }
            let is_present = matches!(app.telemetry, Telemetry::Present { .. });
            let dot_color = if is_present { OK } else { WARN };
            let (dx, dy) = (x as isize - 556, y as isize - 48);
            if dx * dx + dy * dy <= 4 * 4 {
                return dot_color;
            }
            let status_lbl: &[u8] = if is_present { b"ONLINE" } else { b"INVENTORY" };
            // Fitted to 568..686 so the label can never cross the pill rim.
            if font::ink(x, y, 568, 36, SC, status_lbl, 118, dot_color, CARD_RAISED) {
                return dot_color;
            }
            return CARD_RAISED;
        }

        return CARD;
    }

    // Status Band Card
    if (STATUS_Y0..STATUS_Y1).contains(&y) && (STATUS_X0..STATUS_X1).contains(&x) {
        if on_rounded_rect_border(x, y, STATUS_X0, STATUS_Y0, STATUS_X1, STATUS_Y1, STATUS_R) {
            return EDGE;
        }
        if in_rounded_rect(x, y, STATUS_X0, STATUS_Y0, STATUS_X1, STATUS_Y1, STATUS_R) {
            // WHY STATUS_Y0 + 20: with the card 40 rows tall its centre row is
            // 20 below the rim, so the dot sits on the card's centre line rather
            // than 19 rows down inside a 38-row card.
            let (dx, dy) = (x as isize - 46, y as isize - (STATUS_Y0 + 20) as isize);
            if dx * dx + dy * dy <= 3 * 3 {
                return app.status_color;
            }
            // WHY scale 1: the longest status line is 523px at scale 1 and
            // 1046px at scale 2; only the measured run fits the 626px left of
            // the card's inner rim, so the run is fitted into that box.
            // WHY y 104: an 8px inset from the rim puts the 24-row line box
            // 104..128 inside a 96..136 card, which centres it on the 8-point
            // grid the rest of this screen is laid out on.
            if font::ink(x, y, 58, 104, SC, &app.status[..app.status_len], 626, app.status_color, CARD_INNER) {
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

            let icon_hit = match idx {
                0 => icon_chip(x, y, card.x0 + 24, card.y0 + 20),
                1 => icon_channels(x, y, card.x0 + 24, card.y0 + 20),
                2 => icon_power(x, y, card.x0 + 24, card.y0 + 20),
                3 => icon_runtime(x, y, card.x0 + 24, card.y0 + 20),
                _ => None,
            };
            if let Some(c) = icon_hit {
                return c;
            }

            if font::ink(x, y, card.x0 + 40, card.y0 + 14, HSC, card.caption, CAPTION_W, ACCENT, CARD) {
                return ACCENT;
            }
        }
    }

    // Card bodies. The bands come from the card rectangles themselves, so a row
    // can never be drawn outside the card that owns it.
    if (30..690).contains(&x) {
        for (idx, card) in CARDS.iter().enumerate() {
            if (card.y0..card.y1).contains(&y) {
                let hit = match idx {
                    0 => sensor_bus_card(x, y, app),
                    1 => inventory_card(x, y, app),
                    2 => power_card(x, y, app),
                    _ => identity_card(x, y, app),
                };
                return hit.unwrap_or(CARD);
            }
        }
    }

    // Home gesture pill
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }

    BG
}

/// One row of a card, occupying exactly one line box.
///
/// Label on the left, verdict right-aligned at `ROW_X1`, provenance in the
/// middle column between them. The verdict is measured FIRST, so the middle
/// column is always the widest it can be; the provenance is then fitted into
/// that width and can never touch either neighbour or leave the card.
#[inline(never)]
fn row(x: usize, y: usize, top: usize, label: &[u8], verdict: &[u8], color: u32, source: &[u8]) -> Option<u32> {
    if font::ink(x, y, ROW_X0, top, SC, label, LABEL_W, MUTED, CARD) {
        return Some(MUTED);
    }
    let vx = ROW_X1 - font::fit_width(verdict, SC, VERDICT_W);
    if font::ink(x, y, vx, top, SC, verdict, VERDICT_W, color, CARD) {
        return Some(color);
    }
    let sx = ROW_X0 + font::fit_width(label, SC, LABEL_W) + COL_GAP;
    let mid = vx.saturating_sub(COL_GAP).saturating_sub(sx);
    if font::ink(x, y, sx, top, SC, source, mid, FAINT, CARD) {
        return Some(FAINT);
    }
    None
}

/// A standalone full-width line: fitted to the card's inner box so it can never
/// run past the card rim.
#[inline(never)]
fn line(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    if font::ink(x, y, ROW_X0, top, SC, text, ROW_X1 - ROW_X0, color, CARD) {
        return Some(color);
    }
    None
}

/// Card 1: what the sensor bus itself said, quoting the kernel.
#[inline(never)]
fn sensor_bus_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
    let (verdict, color) = match &app.telemetry {
        Telemetry::Present { .. } => (b"reading".as_slice(), OK),
        Telemetry::Absent { .. } => (b"no sensor bound".as_slice(), WARN),
        Telemetry::Denied(_) => (b"refused".as_slice(), WARN),
        Telemetry::Malformed => (b"no data".as_slice(), WARN),
    };
    let mut word = [0u8; 40];
    let mut wn = write_lit(verdict, &mut word);
    if let Telemetry::Denied(e) = &app.telemetry {
        wn += write_lit(b" (", &mut word[wn..]);
        wn += fmt_i64(*e, &mut word[wn..]);
        if wn < word.len() {
            word[wn] = b')';
            wn += 1;
        }
    }
    // Slot 0 is the sensor-read verdict; the branch below owns the slots after
    // it and `slot` carries the live-service row to the first free one.
    //
    // WHY the provenance is the bare call: the verdict of a refused read names
    // the error, and that verdict plus the label leaves 215px of middle column.
    // "sensor_read() -> kernel sensor manager" is 373px wide, so the fitted run
    // lost its tail to an ellipsis. The call alone names the source of the claim
    // and fits every branch this card can draw.
    let top = card_rows_y(0);
    if let Some(c) = row(x, y, top, b"Sensor read", &word[..wn], color, b"sensor_read()") {
        return Some(c);
    }

    let mut slot = 1usize;
    let line_at = |i: usize| top + i * ROW_PITCH;

    match &app.telemetry {
        Telemetry::Absent { status, status_len, error, error_len } => {
            // "Kernel says:" and the kernel's own status string share one row.
            if font::ink(x, y, ROW_X0, line_at(slot), SC, b"Kernel says:", 128, MUTED, CARD) {
                return Some(MUTED);
            }
            if *status_len > 0
                && font::ink(x, y, 176, line_at(slot), SC, &status[..*status_len], ROW_X1 - 176 - 4, WARN, CARD)
            {
                return Some(WARN);
            }
            slot += 1;
            // The kernel's error string is wrapped on word boundaries by the
            // shared face's wrapper and each line is fitted to the full inner
            // width, instead of being cut at a fixed byte count.
            let mut err_lines: [&[u8]; 2] = [&[], &[]];
            let err = &error[..*error_len];
            let n = font::wrap_text(err, SC, ROW_X1 - ROW_X0, 2, &mut err_lines);
            for i in 0..n {
                if font::ink(x, y, ROW_X0, line_at(slot + i), SC, err_lines[i], ROW_X1 - ROW_X0, TEXT, CARD) {
                    return Some(TEXT);
                }
            }
            slot += n;
        }
        Telemetry::Present { lux, accel, near, orientation, orientation_len } => {
            let mut w = [0u8; 32];
            let n = write_lit(b"lux ", &mut w);
            let n = n + fmt_i64(*lux, &mut w[n..]);
            if let Some(c) = row(x, y, line_at(slot), b"Ambient light", &w[..n], OK, b"sensor_read() -> lux") {
                return Some(c);
            }
            slot += 1;
            let mut w = [0u8; 32];
            let n = write_lit(b"accel y ", &mut w);
            let n = n + fmt_i64(*accel, &mut w[n..]);
            if let Some(c) = row(x, y, line_at(slot), b"Accelerometer", &w[..n], OK, b"sensor_read() -> accel") {
                return Some(c);
            }
            slot += 1;
            let near_word: &[u8] = if *near { b"near" } else { b"far" };
            if let Some(c) = row(x, y, line_at(slot), b"Proximity", near_word, OK, b"sensor_read() -> near") {
                return Some(c);
            }
            slot += 1;
            if let Some(c) = row(
                x,
                y,
                line_at(slot),
                b"Orientation",
                &orientation[..*orientation_len],
                OK,
                b"sensor_read() -> orientation",
            ) {
                return Some(c);
            }
            slot += 1;
        }
        Telemetry::Denied(e) => {
            if line(x, y, line_at(slot), status_name(*e), WARN).is_some() {
                return Some(WARN);
            }
            slot += 1;
            if line(x, y, line_at(slot), b"This app holds no sensor control capability.", MUTED).is_some() {
                return Some(MUTED);
            }
            slot += 1;
        }
        Telemetry::Malformed => {
            if line(x, y, line_at(slot), b"Payload carried no present field.", MUTED).is_some() {
                return Some(MUTED);
            }
            slot += 1;
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
    if let Some(c) = row(x, y, line_at(slot), b"Sensor service", &word[..wn], svc_color, b"service_query(11)") {
        return Some(c);
    }
    None
}

/// Card 2: the inventory.
#[inline(never)]
fn inventory_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
    let top = card_rows_y(1);
    macro_rules! channel {
        ($i:expr, $label:expr, $verdict:expr, $color:expr, $reason:expr) => {{
            let top = top + $i * ROW_PITCH;
            if let Some(c) = row(x, y, top, $label, $verdict, $color, $reason) {
                return Some(c);
            }
        }};
    }

    match &app.telemetry {
        Telemetry::Present { .. } => {
            channel!(0, b"Accelerometer", b"measured", OK, b"sensor_read() -> accel, present:true");
            channel!(1, b"Gyroscope", b"no field", WARN, b"sys_sensor_read serialises no gyro axis");
            channel!(2, b"Ambient light", b"measured", OK, b"sensor_read() -> lux, present:true");
            channel!(3, b"Proximity", b"measured", OK, b"sensor_read() -> near, present:true");
        }
        _ => {
            // WHY these are the app's own reasons and not the kernel's verbatim
            // error string this row used to borrow: the middle column of an
            // inventory row is 373px wide at its widest, and the kernel's
            // "No physical IMU or ambient sensor detected" is 414px (the
            // is_hardware_present fallback was 442px). Both were silently
            // ellipsised by the fitted run. The sensor bus card above prints
            // that string in full - wrapped to the card's whole inner width -
            // so the kernel's own words stay on screen; this row says the same
            // thing in a form that fits the column it is measured against.
            let reason: &[u8] = match &app.telemetry {
                Telemetry::Absent { .. } => b"no physical IMU or ambient sensor".as_slice(),
                _ => b"hardware_present is false here".as_slice(),
            };
            channel!(0, b"Accelerometer", b"unavailable", WARN, reason);
            channel!(1, b"Gyroscope", b"unavailable", WARN, b"not in sys_sensor_read payload");
            channel!(2, b"Ambient light", b"unavailable", WARN, reason);
            channel!(3, b"Proximity", b"unavailable", WARN, reason);
        }
    }

    channel!(4, b"Screen orientation", b"unavailable", WARN, b"derived from accel; none present");
    channel!(5, b"Heart rate", b"unavailable", WARN, b"no PPG backend in the sensor manager");
    // WHY "in the manager" here and not "in the sensor manager" as the row
    // above: the barometer label is 2px wider than the heart-rate label and its
    // reason is 42px wider than the PPG one, so the longer form is 1px past the
    // middle column and would be ellipsised.
    channel!(6, b"Barometer", b"unavailable", WARN, b"no pressure backend in the manager");
    channel!(7, b"Compass", b"unavailable", WARN, b"no magnetometer in the sensor manager");

    let (word, color, wn) = match app.inv_sensors {
        Some(code) => {
            let mut w = [0u8; 24];
            let n = write_lit(inventory_word(code), &mut w);
            (w, if code == 0 { OK } else { MUTED }, n)
        }
        None => {
            let mut w = [0u8; 24];
            let n = write_lit(b"refused", &mut w);
            (w, WARN, n)
        }
    };
    if let Some(c) = row(
        x,
        y,
        top + 8 * ROW_PITCH,
        // WHY "Settings" and not "Settings inventory": the label is the widest
        // thing in this row's left column, and at 169px it left only 338px of
        // middle column - 11px too few for the call that names the source. The
        // row's verdict ("unavailable") and its provenance
        // (`settings_hardware_status("sensors")`) both stay, so the row still
        // says exactly which subsystem and which call it is quoting.
        b"Settings",
        &word[..wn],
        color,
        b"settings_hardware_status(\"sensors\")",
    ) {
        return Some(c);
    }

    // WHY it gets its own row slot: it is the last thing the card says, and
    // `line` fits it into the card's full inner width on a character boundary,
    // so a longer variant of this sentence can never run off the card.
    line(x, y, top + 9 * ROW_PITCH, b"Inventory says 'simulated'; the bus says no hardware.", MUTED)
}

/// Card 4: runtime facts.
#[inline(never)]
fn identity_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
    let top = card_rows_y(3);
    let mut v = [0u8; 24];
    let n = fmt_u64(app.task_id, &mut v);
    if let Some(c) = row(x, y, top, b"Task id", &v[..n], TEXT, b"get_task_id()") {
        return Some(c);
    }

    let n = fmt_u64(app.surface_id as u64, &mut v);
    if let Some(c) = row(x, y, top + ROW_PITCH, b"Surface", &v[..n], TEXT, b"surface_create(6, 720x1440, z=200)") {
        return Some(c);
    }

    match app.ticks {
        Probe::Ok(t) => {
            let n = fmt_u64(t / 1000, &mut v);
            if let Some(c) = row(x, y, top + 2 * ROW_PITCH, b"Uptime", &v[..n], TEXT, b"get_ticks() / 1000 = seconds") {
                return Some(c);
            }
        }
        Probe::Refused(e) => {
            let mut w = [0u8; 24];
            let n = write_lit(b"refused (", &mut w);
            let mut n = n + fmt_i64(e, &mut w[n..]);
            if n < w.len() {
                w[n] = b')';
                n += 1;
            }
            if let Some(c) = row(x, y, top + 2 * ROW_PITCH, b"Uptime", &w[..n], WARN, b"get_ticks() refused") {
                return Some(c);
            }
        }
    }
    None
}

/// Card 3: thermal zones and power regulator.
#[inline(never)]
fn power_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
    // Slot counter rather than hard-coded y values: every branch advances it by
    // exactly the number of rows it drew, so the rows of one branch can never
    // overprint the rows of the next, and the inventory row always follows the
    // battery rows. The worst branch (zones + fuel gauge + settings) uses slots
    // 0..=5, and 968 + 5 * 32 + 24 = 1152 stays inside the card (y1 = 1192).
    let top = card_rows_y(2);
    let at = |slot: usize| top + slot * ROW_PITCH;
    let mut slot = 0usize;

    match &app.thermal {
        Thermal::Zones { present, cpu, gpu, battery, modem, charging, state, state_len, throttling } => {
            let mut w = [0u8; 32];
            let mut n = write_lit(b"cpu ", &mut w);
            n = n + fmt_i64(*cpu, &mut w[n..]);
            n = n + write_lit(b" gpu ", &mut w[n..]);
            n = n + fmt_i64(*gpu, &mut w[n..]);
            n = n + write_lit(b" bat ", &mut w[n..]);
            n = n + fmt_i64(*battery, &mut w[n..]);
            n = n + write_lit(b" mod ", &mut w[n..]);
            n = n + fmt_i64(*modem, &mut w[n..]);
            let color = if *present { OK } else { WARN };
            let label: &[u8] = b"Thermal zones";
            if let Some(c) = row(x, y, at(slot), label, &w[..n], color, b"thermal_query()") {
                return Some(c);
            }
            slot += 1;
            let mut w = [0u8; 32];
            let mut n = write_lit(&state[..*state_len], &mut w);
            n = n + write_lit(if *throttling { b" throttling" } else { b" nominal" }, &mut w[n..]);
            if let Some(c) = row(x, y, at(slot), b"Cooling policy", &w[..n], color, b"thermal_query() -> state, throttling") {
                return Some(c);
            }
            slot += 1;
            let mut w = [0u8; 24];
            let n = write_lit(b"charging zone ", &mut w);
            let n = n + fmt_i64(*charging, &mut w[n..]);
            if let Some(c) = row(x, y, at(slot), b"Charging", &w[..n], color, b"thermal_query() -> charging") {
                return Some(c);
            }
            slot += 1;
        }
        Thermal::Refused(e) => {
            let mut w = [0u8; 40];
            let n = write_lit(b"no sensor (", &mut w);
            let n = n + fmt_i64(*e, &mut w[n..]);
            let n = n + write_lit(b" ", &mut w[n..]);
            let n = n + write_lit(status_name(*e), &mut w[n..]);
            // WHY the bare call: a refused thermal read names the error in its
            // verdict, and that verdict ("no sensor (-12 DeviceError)") leaves
            // only 218px of middle column. The 430px form this row used to
            // carry lost more than half of itself to the ellipsis.
            if let Some(c) = row(x, y, at(slot), b"Thermal zones", &w[..n], WARN, b"thermal_query()") {
                return Some(c);
            }
            slot += 1;
        }
        Thermal::Malformed => {
            if let Some(c) = row(x, y, at(slot), b"Thermal zones", b"no data", WARN, b"thermal_query() payload lacked fields") {
                return Some(c);
            }
            slot += 1;
        }
    }

    match &app.battery {
        Battery::Reading { voltage_mv, soc, capacity, charging, charging_len, temp } => {
            let mut w = [0u8; 40];
            let mut n = write_lit(&charging[..*charging_len], &mut w);
            n = n + write_lit(b" ", &mut w[n..]);
            n = n + fmt_i64(*soc, &mut w[n..]);
            n = n + write_lit(b"% ", &mut w[n..]);
            n = n + fmt_i64(*voltage_mv, &mut w[n..]);
            n = n + write_lit(b" mV ", &mut w[n..]);
            n = n + fmt_i64(*temp, &mut w[n..]);
            n = n + write_lit(b" C", &mut w[n..]);
            if let Some(c) = row(x, y, at(slot), b"Fuel gauge", &w[..n], OK, b"battery_query()") {
                return Some(c);
            }
            slot += 1;
            let mut w = [0u8; 24];
            let n = fmt_i64(*capacity, &mut w);
            if let Some(c) = row(x, y, at(slot), b"Capacity", &w[..n], OK, b"battery_query() -> capacity") {
                return Some(c);
            }
        }
        Battery::Absent { governor, governor_len, mhz } => {
            let mut w = [0u8; 40];
            let n = write_lit(b"no fuel gauge", &mut w);
            if let Some(c) = row(x, y, at(slot), b"Battery", &w[..n], WARN, b"battery_query() -> present:false") {
                return Some(c);
            }
            slot += 1;
            let mut w = [0u8; 40];
            let mut n = write_lit(&governor[..*governor_len], &mut w);
            n = n + write_lit(b" ", &mut w[n..]);
            n = n + fmt_i64(*mhz, &mut w[n..]);
            n = n + write_lit(b" MHz", &mut w[n..]);
            if let Some(c) = row(x, y, at(slot), b"DVFS governor", &w[..n], MUTED, b"battery_query() -> governor") {
                return Some(c);
            }
        }
        Battery::Denied(e) => {
            let mut w = [0u8; 40];
            let mut n = write_lit(b"refused (", &mut w);
            n = n + fmt_i64(*e, &mut w[n..]);
            let n = n + write_lit(b" ", &mut w[n..]);
            let n = n + write_lit(status_name(*e), &mut w[n..]);
            if let Some(c) = row(x, y, at(slot), b"Battery", &w[..n], WARN, b"battery_query()") {
                return Some(c);
            }
            slot += 1;
        }
        Battery::Malformed => {
            if let Some(c) = row(x, y, at(slot), b"Battery", b"no data", WARN, b"battery_query()") {
                return Some(c);
            }
            slot += 1;
        }
    }

    let mut w = [0u8; 24];
    let (color, wn) = match app.inv_battery {
        Some(code) => {
            let n = write_lit(inventory_word(code), &mut w);
            (if code == 0 { OK } else { MUTED }, n)
        }
        None => {
            let n = write_lit(b"refused", &mut w);
            (WARN, n)
        }
    };
    // WHY the slot and not a hard-coded row: this row used to be drawn at a
    // literal y = 1216, which is 24px past the bottom of this card (y1 = 1192)
    // and inside the next one. `pixel` dispatches a row to the card whose y-range
    // contains it, so the row was handed to the runtime card, which draws
    // nothing at that y - the settings-inventory row simply never appeared, and
    // the card ended 130px short with no clue why. The slot counter puts it on
    // the same 32px grid as every other row: the worst branch (three thermal
    // rows, two battery rows, this row) ends at 1152, inside the card.
    //
    // WHY the label is "Settings" and not "Inventory, battery": the label is the
    // widest item in this row's left column, and at 163px it left 344px of
    // middle column - too few for the call that names the source of the claim.
    if let Some(c) = row(
        x,
        y,
        at(slot),
        b"Settings",
        &w[..wn],
        color,
        b"settings_hardware_status(\"battery\")",
    ) {
        return Some(c);
    }
    None
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

fn status(app: &mut Dashboard, text: &[u8], color: u32) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
    app.status_color = color;
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[SENSORS (Ring 3)] Sensor inventory starting.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[SENSORS] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut app = Dashboard {
        task_id: std::get_task_id(),
        ticks: probe_ticks(),
        surface_id: surface,
        telemetry: Telemetry::Malformed,
        thermal: Thermal::Malformed,
        battery: Battery::Malformed,
        sensor_service: None,
        inv_display: None,
        inv_sensors: None,
        inv_battery: None,
        inv_wifi: None,
        inv_cellular: None,
        status: [0; 64],
        status_len: 0,
        status_color: MUTED,
    };

    app.telemetry = read_sensors();
    app.thermal = read_thermal();
    app.battery = read_battery();
    app.sensor_service = read_sensor_service();
    app.inv_display = read_inventory("display");
    app.inv_sensors = read_inventory("sensors");
    app.inv_battery = read_inventory("battery");
    app.inv_wifi = read_inventory("wifi");
    app.inv_cellular = read_inventory("cellular");

    let (status_text, status_color): (&[u8], u32) = match &app.telemetry {
        Telemetry::Present { .. } => {
            std::log(0, "[SENSORS] sensor_read() returned live hardware telemetry; channels show real values.").ok();
            (b"live sensor hardware; channels show measured values", ACCENT)
        }
        Telemetry::Absent { .. } => {
            std::log(0, "[SENSORS] sensor_read() reports no sensor hardware is bound; showing the inventory.").ok();
            (b"no sensor hardware bound; inventory below names why", ACCENT)
        }
        Telemetry::Denied(_) => {
            std::log(0, "[SENSORS] sensor_read() refused: this app holds no sensor control capability.").ok();
            (b"sensor read refused; every channel unavailable", ACCENT)
        }
        Telemetry::Malformed => {
            std::log(0, "[SENSORS] sensor_read() returned a payload with no present field.").ok();
            (b"sensor payload had no present field", ACCENT)
        }
    };
    status(&mut app, status_text, status_color);

    if app.inv_wifi == Some(2) {
        std::log(0, "[SENSORS] settings inventory: no physical Wi-Fi controller on this platform.").ok();
    }
    if app.inv_cellular == Some(2) {
        std::log(0, "[SENSORS] settings inventory: no physical baseband modem on this platform.").ok();
    }
    if app.inv_display == Some(0) {
        std::log(0, "[SENSORS] settings inventory: display scanout is available.").ok();
    }

    let mut v = [0u8; 24];
    let n = write_lit(BUILD_VERSION.as_bytes(), &mut v);
    std::log(0, "[SENSORS] Inventory app build: ").ok();
    let _ = n;

    repaint(surface, &app, Dirty::All);
    let _ = std::surface_flip(6);
    std::log(0, "[SENSORS] Initial frame presented to hardware scanout.").ok();

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
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    if matches!(input.kind, 4 | 6 | 7 | 8 | 10) {
                        std::log(0, "[SENSORS] Navigation gesture. Exiting to launcher.").ok();
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
                        app.telemetry = read_sensors();
                        app.thermal = read_thermal();
                        app.battery = read_battery();
                        app.sensor_service = read_sensor_service();
                        // Re-read the tick probe with the rest, so the Uptime
                        // row reflects this refresh rather than the boot frame.
                        // Leaving it out published a fresh reading while the
                        // surface kept the start-up value.
                        app.ticks = probe_ticks();
                        app.inv_display = read_inventory("display");
                        app.inv_sensors = read_inventory("sensors");
                        app.inv_battery = read_inventory("battery");
                        app.inv_wifi = read_inventory("wifi");
                        app.inv_cellular = read_inventory("cellular");
                        status(&mut app, b"re-read sensor bus, thermal zones and fuel gauge", ACCENT);
                        std::log(0, "[SENSORS] Re-read sensor bus, thermal zones and fuel gauge.").ok();
                        dirty = dirty.widen(Dirty::Refresh);
                    }
                }
                if dirty != Dirty::None {
                    repaint(surface, &app, dirty);
                    let _ = std::surface_flip(6);
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] Sensor dashboard encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
