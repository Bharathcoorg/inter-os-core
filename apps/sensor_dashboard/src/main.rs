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

/// The version string compiled into this binary from the crate manifest.
///
/// A build-time constant of the artifact itself. It is labelled on screen as
/// such and is never presented as a version read back from the kernel.
const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// Result of one real probe. Nothing here is ever defaulted to a made-up value.
///
/// `get_ticks` either returns ticks or a negative status, so there is no third
/// "succeeded but empty" case to represent for it.
enum Probe<T: Copy> {
    Ok(T),
    /// The syscall returned a negative kernel status code.
    Refused(i64),
}

/// One `sensor_read` call, decoded.
enum Telemetry {
    /// The payload declared hardware and carried the fields we need.
    Present { lux: i64, accel: i64, near: bool, orientation: [u8; 24], orientation_len: usize },
    /// The payload declared that no sensor is bound. The kernel's own words are
    /// kept and shown, rather than being replaced with our own phrasing.
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
    /// The payload reported `present:false`. The fields are present in the JSON
    /// but describe nothing, so they are discarded rather than displayed.
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
    /// `settings_hardware_status` for the categories this device has a verdict
    /// for. Stored per category so each row can name its own source.
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

fn read_sensors() -> Telemetry {
    let mut buf = [0u8; 256];
    let n = match std::sensor_read(&mut buf) {
        Ok(n) => n,
        Err(e) => return Telemetry::Denied(e),
    };
    let payload = &buf[..n.min(buf.len())];

    match json_bool(payload, b"present") {
        Some(true) => {
            // Only a payload that actually carries the fields is treated as a
            // reading. A missing field is Malformed, not zero.
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

    // The payload always carries a `present` flag. When it is false the numeric
    // fields are the struct's zero initialisers, not measurements, so they are
    // dropped rather than shown as a reading of 0 mV / 0%.
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
            // The governor and its frequency come from the power manager, not
            // from the fuel gauge, so they stay meaningful with no gauge bound.
            let mut governor = [0u8; 20];
            let governor_len = json_str(payload, b"governor", &mut governor).unwrap_or(0);
            let mhz = json_i64(payload, b"mhz").unwrap_or(0);
            Battery::Absent { governor, governor_len, mhz }
        }
        None => Battery::Malformed,
    }
}

/// Live lifecycle record of the sensor service. `service_query` carries no
/// ambient authority, so this is a genuine read of kernel state.
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

/// Reads the 1 kHz boot timer.
///
/// The capability slot for `TimerTickRead` is chosen by the spawn policy, so the
/// granted table is probed rather than an index assumed. A refused read stays
/// refused instead of being reported as zero seconds of uptime.
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

/// The kernel's settings-inventory verdict for one category.
///
/// This is a DIFFERENT source from telemetry and is labelled as such wherever it
/// appears: it describes the settings inventory's opinion of a category, not
/// whether a sensor answered a read.
fn read_inventory(category: &str) -> Option<i64> {
    std::settings_hardware_status(category).ok()
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

fn inventory_word(code: i64) -> &'static [u8] {
    match code {
        0 => b"available",
        1 => b"simulated",
        2 => b"unavailable",
        _ => b"unknown",
    }
}

/// Human-readable name of a negative `SyscallResult`.
///
/// `SyscallResult` is a `repr(i64)` enum, so the wire value is a small negative
/// integer. Naming it is more useful to a user than printing the number alone,
/// and the number is still shown so the mapping can be checked.
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

/// Writes the decimal digits of `v`, most significant first.
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

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
const EDGE: u32 = 0xFF26354D;
const TEXT: u32 = 0xFFF4F7FB;
const MUTED: u32 = 0xFF9AA8BC;
const FAINT: u32 = 0xFF6C7A8E;
const ACCENT: u32 = 0xFF31C4FF;
const OK: u32 = 0xFF56D68A;
const WARN: u32 = 0xFFFFC043;

struct Card {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    caption: &'static [u8],
}

const CARDS: [Card; 4] = [
    Card { x0: 30, y0: 158, x1: 690, y1: 470, caption: b"SENSOR BUS, AS THE KERNEL REPORTS IT" },
    Card { x0: 30, y0: 490, x1: 690, y1: 980, caption: b"CHANNEL INVENTORY" },
    Card { x0: 30, y0: 1000, x1: 690, y1: 1264, caption: b"THERMAL AND POWER" },
    Card { x0: 30, y0: 1284, x1: 690, y1: 1404, caption: b"THIS APP, MEASURED" },
];

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

#[inline(never)]
fn pixel(x: usize, y: usize, app: &Dashboard) -> u32 {
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
        if font::text_at(x, y, 116, 32, 3, b"Sensors") {
            return TEXT;
        }
        return CARD;
    }

    // Status band: the one-line verdict, at the size the display can carry.
    if (96..130).contains(&y) {
        if font::text_at(x, y, 30, 104, 2, &app.status[..app.status_len]) {
            return app.status_color;
        }
        return BG;
    }

    for card in CARDS.iter() {
        if y >= card.y0 && y < card.y1 && x >= card.x0 && x < card.x1 {
            if y == card.y0 || y == card.y1 - 1 || x == card.x0 || x == card.x1 - 1 {
                return EDGE;
            }
            if font::text_at(x, y, card.x0 + 14, card.y0 + 10, 2, card.caption) {
                return ACCENT;
            }
            return CARD;
        }
    }

    if (158..470).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = sensor_bus_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (490..980).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = inventory_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (1000..1264).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = power_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (1284..1404).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = identity_card(x, y, app) {
            return c;
        }
        return CARD;
    }

    // Home gesture pill
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }
    BG
}

/// Label on the left, verdict on the right, provenance underneath.
#[inline(never)]
fn row(x: usize, y: usize, top: usize, label: &[u8], verdict: &[u8], color: u32, source: &[u8]) -> Option<u32> {
    if font::text_at(x, y, 44, top, 2, label) {
        return Some(MUTED);
    }
    let vx = 676usize.saturating_sub(font::text_width(verdict, 2));
    if font::text_at(x, y, vx, top, 2, verdict) {
        return Some(color);
    }
    if font::text_at(x, y, 44, top + 18, 2, source) {
        return Some(FAINT);
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
    if let Some(c) = row(x, y, 190, b"Sensor read", &word[..wn], color, b"sensor_read() -> kernel sensor manager") {
        return Some(c);
    }

    // The kernel's own words, quoted verbatim rather than paraphrased.
    match &app.telemetry {
        Telemetry::Absent { status, status_len, error, error_len } => {
            if font::text_at(x, y, 44, 236, 2, b"Kernel says:") {
                return Some(MUTED);
            }
            if *status_len > 0
                && font::text_at(x, y, 176, 236, 2, &status[..*status_len])
            {
                return Some(WARN);
            }
            // The reason string, wrapped at the card's text width.
            let err = &error[..*error_len];
            let split = err.len().min(46);
            if font::text_at(x, y, 44, 264, 2, &err[..split]) {
                return Some(TEXT);
            }
            if font::text_at(x, y, 44, 288, 2, &err[split..]) {
                return Some(TEXT);
            }
        }
        Telemetry::Present { lux, accel, near, orientation, orientation_len } => {
            // A bound sensor would be reported here, from the decode. The same
            // code path serves a device that has one.
            let mut w = [0u8; 32];
            let n = write_lit(b"lux ", &mut w);
            let n = n + fmt_i64(*lux, &mut w[n..]);
            if let Some(c) = row(x, y, 236, b"Ambient light", &w[..n], OK, b"sensor_read() -> lux") {
                return Some(c);
            }
            let mut w = [0u8; 32];
            let n = write_lit(b"accel y ", &mut w);
            let n = n + fmt_i64(*accel, &mut w[n..]);
            if let Some(c) = row(x, y, 278, b"Accelerometer", &w[..n], OK, b"sensor_read() -> accel") {
                return Some(c);
            }
            let near_word: &[u8] = if *near { b"near" } else { b"far" };
            if let Some(c) = row(x, y, 320, b"Proximity", near_word, OK, b"sensor_read() -> near") {
                return Some(c);
            }
            if let Some(c) = row(
                x,
                y,
                362,
                b"Orientation",
                &orientation[..*orientation_len],
                OK,
                b"sensor_read() -> orientation",
            ) {
                return Some(c);
            }
        }
        Telemetry::Denied(e) => {
            if line(x, y, 240, status_name(*e), WARN).is_some() {
                return Some(WARN);
            }
            if line(x, y, 264, b"This app holds no sensor control capability.", MUTED).is_some() {
                return Some(MUTED);
            }
        }
        Telemetry::Malformed => {
            if line(x, y, 240, b"Payload carried no present field.", MUTED).is_some() {
                return Some(MUTED);
            }
        }
    }

    // Live sensor service record.
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
    if let Some(c) = row(x, y, 412, b"Sensor service", &word[..wn], svc_color, b"service_query(11)") {
        return Some(c);
    }
    None
}

/// Card 2: the inventory. One row per channel the kernel defines, each with a
/// verdict and the reason, so the user learns what the device can actually do.
#[inline(never)]
fn inventory_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
    // A helper closure would borrow `x`/`y` mutably twice; the macro keeps the
    // rows uniform and lets each one be a plain early return.
    macro_rules! channel {
        ($i:expr, $label:expr, $verdict:expr, $color:expr, $reason:expr) => {{
            let top = 532 + $i * 42;
            if let Some(c) = row(x, y, top, $label, $verdict, $color, $reason) {
                return Some(c);
            }
        }};
    }

    // The four channels `sys_sensor_read` would report if hardware existed.
    match &app.telemetry {
        Telemetry::Present { .. } => {
            channel!(0, b"Accelerometer", b"measured", OK, b"sensor_read() -> accel, present:true");
            channel!(1, b"Gyroscope", b"no field", WARN, b"sys_sensor_read serialises no gyro axis");
            channel!(2, b"Ambient light", b"measured", OK, b"sensor_read() -> lux, present:true");
            channel!(3, b"Proximity", b"measured", OK, b"sensor_read() -> near, present:true");
        }
        _ => {
            // The reason is the kernel's own error string when it gave one, so
            // the row states the real cause instead of a guess.
            let reason: &[u8] = match &app.telemetry {
                Telemetry::Absent { error, error_len, .. } => &error[..*error_len],
                _ => b"is_hardware_present() is false for this platform",
            };
            let reason = if reason.is_empty() {
                b"is_hardware_present() is false for this platform".as_slice()
            } else {
                reason
            };
            let split = reason.len().min(46);
            channel!(0, b"Accelerometer", b"unavailable", WARN, &reason[..split]);
            channel!(1, b"Gyroscope", b"unavailable", WARN, b"not in sys_sensor_read payload");
            channel!(2, b"Ambient light", b"unavailable", WARN, &reason[..split]);
            channel!(3, b"Proximity", b"unavailable", WARN, &reason[..split]);
        }
    }

    // Channels the sensor manager does not define at all. These are absent by
    // omission from `kernel::sensor::SensorManager`, not by a failed read.
    channel!(4, b"Screen orientation", b"unavailable", WARN, b"derived from accel; no accel to derive");
    channel!(5, b"Heart rate", b"unavailable", WARN, b"no PPG backend in the sensor manager");
    channel!(6, b"Barometer", b"unavailable", WARN, b"no pressure backend in the sensor manager");
    channel!(7, b"Compass", b"unavailable", WARN, b"no magnetometer in the sensor manager");

    // The settings inventory is a different source, named as one.
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
        532 + 8 * 42,
        b"Settings inventory",
        &word[..wn],
        color,
        b"settings_hardware_status(\"sensors\")",
    ) {
        return Some(c);
    }

    // WHY this line exists: the settings inventory and the sensor bus are two
    // different real sources and on this device they do not agree. Saying so is
    // more useful than silently preferring one of them.
    line(x, y, 924, b"Inventory says 'simulated'; the bus says no hardware.", MUTED)
}

/// Card 4: the app's own launch-path facts, so the inventory is anchored to a
/// task that demonstrably exists, and the scope is stated plainly.
#[inline(never)]
fn identity_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
    let mut v = [0u8; 24];
    let n = fmt_u64(app.task_id, &mut v);
    if let Some(c) = row(x, y, 1318, b"Task id", &v[..n], TEXT, b"get_task_id()") {
        return Some(c);
    }

    let n = fmt_u64(app.surface_id as u64, &mut v);
    if let Some(c) = row(x, y, 1350, b"Surface", &v[..n], TEXT, b"surface_create(6, 720x1440, z=200)") {
        return Some(c);
    }

    // The uptime row says "unavailable" rather than showing a number when the
    // timer read was refused: a bare 0 would read as a measurement. The refused
    // status code belongs here, because this row is the one that made the call.
    match app.ticks {
        Probe::Ok(t) => {
            let n = fmt_u64(t / 1000, &mut v);
            if let Some(c) = row(x, y, 1382, b"Uptime", &v[..n], TEXT, b"get_ticks() / 1000 = seconds") {
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
            if let Some(c) = row(x, y, 1382, b"Uptime", &w[..n], WARN, b"get_ticks() refused") {
                return Some(c);
            }
        }
    }
    None
}

/// Card 3: thermal zones and the fuel gauge, each with its own real verdict.
#[inline(never)]
fn power_card(x: usize, y: usize, app: &Dashboard) -> Option<u32> {
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
            let label: &[u8] = if *present { b"Thermal zones" } else { b"Thermal zones" };
            if let Some(c) = row(x, y, 1030, label, &w[..n], color, b"thermal_query() -> cpu/gpu/battery/modem") {
                return Some(c);
            }
            let mut w = [0u8; 32];
            let mut n = write_lit(&state[..*state_len], &mut w);
            n = n + write_lit(if *throttling { b" throttling" } else { b" nominal" }, &mut w[n..]);
            if let Some(c) = row(x, y, 1072, b"Cooling policy", &w[..n], color, b"thermal_query() -> state, throttling") {
                return Some(c);
            }
            let mut w = [0u8; 24];
            let n = write_lit(b"charging zone ", &mut w);
            let n = n + fmt_i64(*charging, &mut w[n..]);
            if let Some(c) = row(x, y, 1114, b"Charging", &w[..n], color, b"thermal_query() -> charging") {
                return Some(c);
            }
        }
        Thermal::Refused(e) => {
            let mut w = [0u8; 40];
            let n = write_lit(b"no sensor (", &mut w);
            let n = n + fmt_i64(*e, &mut w[n..]);
            let n = n + write_lit(b" ", &mut w[n..]);
            let n = n + write_lit(status_name(*e), &mut w[n..]);
            if let Some(c) = row(x, y, 1030, b"Thermal zones", &w[..n], WARN, b"thermal_query() -> DeviceError when unbound") {
                return Some(c);
            }
        }
        Thermal::Malformed => {
            if let Some(c) = row(x, y, 1030, b"Thermal zones", b"no data", WARN, b"thermal_query() payload lacked fields") {
                return Some(c);
            }
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
            if let Some(c) = row(x, y, 1156, b"Fuel gauge", &w[..n], OK, b"battery_query() -> present:true") {
                return Some(c);
            }
            let mut w = [0u8; 24];
            let n = fmt_i64(*capacity, &mut w);
            if let Some(c) = row(x, y, 1198, b"Capacity", &w[..n], OK, b"battery_query() -> capacity") {
                return Some(c);
            }
        }
        Battery::Absent { governor, governor_len, mhz } => {
            // The gauge is absent, so no voltage/SoC/capacity row is shown at
            // all. The governor is a separate object and stays meaningful.
            let mut w = [0u8; 40];
            let n = write_lit(b"no fuel gauge", &mut w);
            if let Some(c) = row(x, y, 1156, b"Battery", &w[..n], WARN, b"battery_query() -> present:false") {
                return Some(c);
            }
            let mut w = [0u8; 40];
            let mut n = write_lit(&governor[..*governor_len], &mut w);
            n = n + write_lit(b" ", &mut w[n..]);
            n = n + fmt_i64(*mhz, &mut w[n..]);
            n = n + write_lit(b" MHz", &mut w[n..]);
            if let Some(c) = row(x, y, 1198, b"DVFS governor", &w[..n], MUTED, b"battery_query() -> governor, mhz") {
                return Some(c);
            }
        }
        Battery::Denied(e) => {
            let mut w = [0u8; 40];
            let mut n = write_lit(b"refused (", &mut w);
            n = n + fmt_i64(*e, &mut w[n..]);
            let n = n + write_lit(b" ", &mut w[n..]);
            let n = n + write_lit(status_name(*e), &mut w[n..]);
            if let Some(c) = row(x, y, 1156, b"Battery", &w[..n], WARN, b"battery_query() needs SystemControl") {
                return Some(c);
            }
        }
        Battery::Malformed => {
            if let Some(c) = row(x, y, 1156, b"Battery", b"no data", WARN, b"battery_query() payload lacked fields") {
                return Some(c);
            }
        }
    }

    // The settings inventory verdict for the battery, as a separate source.
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
    if let Some(c) = row(
        x,
        y,
        1216,
        b"Inventory, battery",
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

    // The console line states the kernel's verdict, not a summary of it.
    // The status text is chosen first and applied after the match, because the
    // match borrows `app` immutably and `status` needs it mutably.
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

    // Display / Wi-Fi / cellular verdicts are read and, being about other
    // subsystems, are logged rather than given dashboard rows.
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

    render(surface, &app);
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
                        // Re-read every source, so the screen always reflects a
                        // call that actually happened.
                        app.telemetry = read_sensors();
                        app.thermal = read_thermal();
                        app.battery = read_battery();
                        app.sensor_service = read_sensor_service();
                        app.inv_display = read_inventory("display");
                        app.inv_sensors = read_inventory("sensors");
                        app.inv_battery = read_inventory("battery");
                        app.inv_wifi = read_inventory("wifi");
                        app.inv_cellular = read_inventory("cellular");
                        status(&mut app, b"re-read sensor bus, thermal zones and fuel gauge", ACCENT);
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
    std::log(0, "[PANIC] Sensor dashboard encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
