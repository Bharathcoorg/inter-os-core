//! InterOS Core Open-Source Reference App: Camera Preview
//!
//! Reports the state of the camera surface honestly.
//!
//! QEMU binds no MIPI-CSI-2 image sensor, so there is nothing to preview. This
//! app does not draw a fake viewfinder with a fake photograph in it. Instead it
//! renders, from real kernel calls:
//!
//! - `service_query(8)`  -> the camera service's live lifecycle record
//! - `service_list()`    -> the same service's heartbeat age and failure count
//! - `camera_capture()`  -> the actual answer, whatever it is
//!
//! The viewfinder rectangle is drawn as an explicitly empty surface with a
//! "no signal" mark and the reason text taken from the call that failed. Every
//! control that cannot work is drawn recessed and says why. If a real frame ever
//! does arrive, the same rectangle blits the returned pixels and labels the
//! width, height and scene the kernel reported.

#![no_std]
#![no_main]

mod font;

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Service id of the camera service in the kernel service registry.
const CAMERA_SERVICE_ID: u8 = 8;

/// Frame buffer this app can accept, in bytes.
const FRAME_BUF_BYTES: usize = 16384;

/// Staging buffer for one frame. Kept out of the stack: a Ring-3 stack frame of
/// this size would be unreasonable.
static mut FRAME_BUF: [u8; FRAME_BUF_BYTES] = [0; FRAME_BUF_BYTES];

// -----------------------------------------------------------------------------
// Measured state
// -----------------------------------------------------------------------------

/// What one `camera_capture` attempt actually returned.
#[derive(Clone, Copy)]
enum Capture {
    Got { bytes: usize, width: u32, height: u32, scene: u32, frame_id: u32 },
    Denied(i64),
    NoSensor(i64),
    TooSmall(i64),
    Empty,
    Malformed,
}

impl Capture {
    fn headline(&self) -> &'static [u8] {
        match self {
            Capture::Got { .. } => b"frame received",
            Capture::Denied(_) => b"refused",
            Capture::NoSensor(_) => b"no sensor",
            Capture::TooSmall(_) => b"buffer too small",
            Capture::Empty => b"empty frame",
            Capture::Malformed => b"no metadata",
        }
    }
}

struct CameraPreview {
    capture: Capture,
    service_state: Option<(u8, u32, bool)>,
    service_heartbeat: Option<(u64, u32)>,
    autofocus: Option<bool>,
    flash: Option<bool>,
    status: [u8; 64],
    status_len: usize,
    status_color: u32,
}

// -----------------------------------------------------------------------------
// Real sources
// -----------------------------------------------------------------------------

fn probe_capture() -> Capture {
    let mut meta = [0u32; 4];
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(FRAME_BUF) };
    let result = std::camera_capture(buf, &mut meta);
    match result {
        Ok(0) => Capture::Empty,
        Ok(bytes) => {
            if meta[0] == 0 || meta[1] == 0 {
                return Capture::Malformed;
            }
            let needed = meta[0] as usize * meta[1] as usize * 4;
            if bytes < needed || needed > FRAME_BUF_BYTES {
                return Capture::TooSmall(0);
            }
            Capture::Got { bytes, width: meta[0], height: meta[1], scene: meta[2], frame_id: meta[3] }
        }
        Err(-3) => Capture::TooSmall(-3),
        Err(-12) => Capture::NoSensor(-12),
        Err(e) => Capture::Denied(e),
    }
}

fn read_camera_service() -> Option<(u8, u32, bool)> {
    let mut out = [0u8; 12];
    match std::service_query(CAMERA_SERVICE_ID, &mut out) {
        Ok(n) if n >= 12 => Some((
            out[1],
            u32::from_le_bytes([out[6], out[7], out[8], out[9]]),
            out[3] != 0,
        )),
        _ => None,
    }
}

fn read_camera_heartbeat() -> Option<(u64, u32)> {
    let mut buf = [0u8; 512];
    let n = std::service_list(&mut buf).ok()?;
    let snapshot = &buf[..n.min(buf.len())];
    let mut line_start = 0usize;
    while line_start < snapshot.len() {
        let mut line_end = line_start;
        while line_end < snapshot.len() && snapshot[line_end] != b'\n' {
            line_end += 1;
        }
        let line = &snapshot[line_start..line_end];
        let mut fields: [&[u8]; 8] = [b""; 8];
        let mut count = 0usize;
        let mut seg_start = 0usize;
        for i in 0..=line.len() {
            if i == line.len() || line[i] == b'|' {
                if count < 8 {
                    fields[count] = &line[seg_start..i];
                }
                count += 1;
                seg_start = i + 1;
            }
        }
        if count == 8 && fields[0] == b"8" {
            return Some((parse_u64(fields[6]), parse_u64(fields[7]) as u32));
        }
        line_start = line_end + 1;
    }
    None
}

fn parse_u64(s: &[u8]) -> u64 {
    let mut v = 0u64;
    let mut digits = 0;
    for &c in s {
        if !c.is_ascii_digit() {
            break;
        }
        v = v.saturating_mul(10).saturating_add((c - b'0') as u64);
        digits += 1;
    }
    if digits == 0 {
        0
    } else {
        v
    }
}

fn read_autofocus() -> Option<bool> {
    std::camera_autofocus_state().ok()
}

fn toggle_flash() -> Option<bool> {
    std::camera_toggle_flash().ok()
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

fn scene_word(scene: u32) -> &'static [u8] {
    match scene {
        0 => b"general",
        1 => b"document",
        2 => b"barcode/qr",
        3 => b"portrait",
        4 => b"night",
        5 => b"landscape",
        _ => b"unclassified",
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

fn capture_reason(capture: &Capture) -> ([u8; 48], usize) {
    let mut out = [0u8; 48];
    let code = match capture {
        Capture::Denied(e) => Some(*e),
        Capture::NoSensor(e) => Some(*e),
        Capture::TooSmall(e) => Some(*e),
        _ => None,
    };
    let n = match code {
        Some(e) => {
            let mut n = write_lit(b"kernel status ", &mut out);
            n += fmt_i64(e, &mut out[n..]);
            n
        }
        None => write_lit(capture.headline(), &mut out),
    };
    (out, n)
}

// -----------------------------------------------------------------------------
// Palette and geometry
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
const CARD_INNER: u32 = 0xFF0B121E;
const RECESSED: u32 = 0xFF0D1522;
const EDGE: u32 = 0xFF26354D;
const EDGE_MUTED: u32 = 0xFF1B2536;
const TEXT: u32 = 0xFFF4F7FB;
const MUTED: u32 = 0xFF9AA8BC;
const FAINT: u32 = 0xFF6C7A8E;
const ACCENT: u32 = 0xFF31C4FF;
const OK: u32 = 0xFF56D68A;
const WARN: u32 = 0xFFFFC043;

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

/// Vector Camera Lens / Aperture icon for header.
fn icon_lens(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq <= 16 * 16 {
        // Outer bezel ring
        if dist_sq >= 14 * 14 {
            return Some(ACCENT);
        }
        // Inner lens ring
        if dist_sq >= 7 * 7 && dist_sq <= 9 * 9 {
            return Some(MUTED);
        }
        // Center aperture pupil
        if dist_sq <= 3 * 3 {
            return Some(CARD_INNER);
        }
        // Specular highlight streak
        if (dx + dy).abs() <= 1 && dx <= -3 && dy <= -3 {
            return Some(TEXT);
        }
        return Some(CARD_RAISED);
    }
    None
}

/// Vector Server / Microchip icon for Card 0.
fn icon_service(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    if dx.abs() <= 9 && dy.abs() <= 7 {
        if dx.abs() == 9 || dy.abs() == 7 || dy == 0 {
            return Some(ACCENT);
        }
        // Status LEDs
        if dy == -3 && (dx == -5 || dx == -2) {
            return Some(OK);
        }
        if dy == 4 && (dx == -5 || dx == -2) {
            return Some(WARN);
        }
        return Some(CARD_INNER);
    }
    None
}

/// Vector Reticle / Viewfinder icon for Card 1.
fn icon_reticle(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    // 4 Corner brackets: 16x16 box
    if dx.abs() <= 8 && dy.abs() <= 8 {
        let is_bracket = (dx.abs() >= 5 && (dy.abs() == 8 || dy.abs() == 7))
            || (dy.abs() >= 5 && (dx.abs() == 8 || dx.abs() == 7));
        if is_bracket {
            return Some(ACCENT);
        }
        // Center crosshair
        if (dx.abs() <= 2 && dy == 0) || (dy.abs() <= 2 && dx == 0) {
            return Some(TEXT);
        }
    }
    None
}

/// Vector Shutter / Controls icon for Card 2.
fn icon_shutter(x: usize, y: usize, cx: usize, cy: usize) -> Option<u32> {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let dist_sq = dx * dx + dy * dy;
    if dist_sq <= 10 * 10 {
        if dist_sq >= 8 * 8 {
            return Some(WARN);
        }
        if dist_sq <= 3 * 3 {
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

const CARDS: [Card; 3] = [
    Card { x0: 30, y0: 154, x1: 690, y1: 460, caption: b"CAMERA SERVICE (SERVICE 8)" },
    Card { x0: 30, y0: 472, x1: 690, y1: 952, caption: b"VIEWFINDER SURFACE" },
    Card { x0: 30, y0: 964, x1: 690, y1: 1312, caption: b"SENSOR CONTROLS & ACTUATORS" },
];

/// Viewfinder rectangle inside its card.
const VX0: usize = 60;
const VX1: usize = 660;
const VY0: usize = 520;
const VY1: usize = 856;

/// Control geometry.
const BTN_Y: usize = 1024;
const BTN_H: usize = 64;
const BTN_W: usize = 196;
const BTN_X: [usize; 3] = [44, 262, 480];
const REPROBE_Y: usize = 1144;
const REPROBE_H: usize = 56;

const SHUTTER: usize = 0;
const AF_LOCK: usize = 1;
const FLASH: usize = 2;

fn button_label(idx: usize) -> &'static [u8] {
    match idx {
        SHUTTER => b"SHUTTER",
        AF_LOCK => b"AF LOCK",
        _ => b"FLASH",
    }
}

fn control_enabled(app: &CameraPreview, idx: usize) -> bool {
    match idx {
        SHUTTER => matches!(app.capture, Capture::Got { .. }),
        AF_LOCK => app.autofocus.is_some(),
        _ => app.flash.is_some(),
    }
}

fn control_reason(app: &CameraPreview, idx: usize) -> &'static [u8] {
    if control_enabled(app, idx) {
        return b"";
    }
    match idx {
        SHUTTER => match app.capture {
            Capture::Denied(_) => b"capture capability not granted",
            Capture::NoSensor(_) => b"no image sensor is bound",
            Capture::TooSmall(_) => b"frame exceeds this app's buffer",
            Capture::Empty => b"the sensor returned no pixels",
            _ => b"no frame has been returned yet",
        },
        AF_LOCK => b"no autofocus actuator is bound",
        _ => b"no flash torch is bound",
    }
}

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
fn center_line(x: usize, y: usize, top: usize, text: &[u8], color: u32) -> Option<u32> {
    let x0 = font::text_center(VX0, VX1, 2, text);
    if font::text_at(x, y, x0, top, 2, text) {
        return Some(color);
    }
    None
}

fn render(surface: u32, app: &CameraPreview) {
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
fn pixel(x: usize, y: usize, app: &CameraPreview) -> u32 {
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

        // Camera Lens Icon
        if let Some(c) = icon_lens(x, y, 134, 48) {
            return c;
        }

        // App Title and Subtitle
        if font::text_at(x, y, 164, 32, 3, b"Camera Inspector") {
            return TEXT;
        }
        if font::text_at(x, y, 166, 62, 1, b"MIPI-CSI-2 SURFACE DIAGNOSTICS") {
            return FAINT;
        }

        // Status pill on top right
        if in_rounded_rect(x, y, 540, 34, 690, 62, 6) {
            if on_rounded_rect_border(x, y, 540, 34, 690, 62, 6) {
                return EDGE;
            }
            let is_got = matches!(app.capture, Capture::Got { .. });
            let dot_color = if is_got { OK } else { WARN };
            let (dx, dy) = (x as isize - 556, y as isize - 48);
            if dx * dx + dy * dy <= 4 * 4 {
                return dot_color;
            }
            let status_lbl: &[u8] = if is_got { b"LIVE" } else { b"INSPECT" };
            if font::text_at(x, y, 568, 41, 2, status_lbl) {
                return dot_color;
            }
            return CARD_RAISED;
        }

        return CARD;
    }

    // Status Band Card (y: 100..144)
    if (100..144).contains(&y) && (30..690).contains(&x) {
        if on_rounded_rect_border(x, y, 30, 100, 690, 144, 6) {
            return EDGE;
        }
        if in_rounded_rect(x, y, 30, 100, 690, 144, 6) {
            let (dx, dy) = (x as isize - 46, y as isize - 122);
            if dx * dx + dy * dy <= 3 * 3 {
                return app.status_color;
            }
            if font::text_at(x, y, 58, 114, 2, &app.status[..app.status_len]) {
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
                0 => icon_service(x, y, card.x0 + 24, card.y0 + 20),
                1 => icon_reticle(x, y, card.x0 + 24, card.y0 + 20),
                2 => icon_shutter(x, y, card.x0 + 24, card.y0 + 20),
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

    if (154..460).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = service_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (472..952).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = viewfinder_card(x, y, app) {
            return c;
        }
        return CARD;
    }
    if (964..1312).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = controls_card(x, y, app) {
            return c;
        }
        return CARD;
    }

    // Home gesture bar
    if (1418..1424).contains(&y) && (260..460).contains(&x) {
        return MUTED;
    }

    BG
}

#[inline(never)]
fn service_card(x: usize, y: usize, app: &CameraPreview) -> Option<u32> {
    let mut word = [0u8; 32];
    let (color, wn) = match app.service_state {
        Some((2, gen, _)) => {
            let n = write_lit(service_state_word(2), &mut word);
            let n = n + write_lit(b" gen ", &mut word[n..]);
            (OK, n + fmt_u64(gen as u64, &mut word[n..]))
        }
        Some((state, gen, critical)) => {
            let n = write_lit(service_state_word(state), &mut word);
            let mut n = n + write_lit(b" gen ", &mut word[n..]);
            n += fmt_u64(gen as u64, &mut word[n..]);
            if critical {
                n += write_lit(b" critical", &mut word[n..]);
            }
            (WARN, n)
        }
        None => (WARN, write_lit(b"no record", &mut word)),
    };
    if let Some(c) = field(x, y, 186, b"Service state", &word[..wn], color) {
        return Some(c);
    }
    if line(x, y, 206, b"service_query(8)", FAINT).is_some() {
        return Some(FAINT);
    }

    let (hb_color, hb_text) = match app.service_heartbeat {
        Some((age, failures)) => {
            let mut v = [0u8; 40];
            let n = write_lit(b"age ", &mut v);
            let n = n + fmt_u64(age, &mut v[n..]);
            let n = n + write_lit(b" ticks, ", &mut v[n..]);
            let n = n + fmt_u64(failures as u64, &mut v[n..]);
            let n = n + write_lit(b" fails", &mut v[n..]);
            (if failures == 0 { OK } else { WARN }, (v, n))
        }
        None => {
            let mut v = [0u8; 40];
            let n = write_lit(b"not published", &mut v);
            (WARN, (v, n))
        }
    };
    if let Some(c) = field(x, y, 242, b"Heartbeat", &hb_text.0[..hb_text.1], hb_color) {
        return Some(c);
    }
    if line(x, y, 262, b"service_list() heartbeat age / failures", FAINT).is_some() {
        return Some(FAINT);
    }

    let mut word = [0u8; 32];
    let (sensor_color, wn) = match app.capture {
        Capture::Got { .. } => {
            let n = write_lit(b"yes, a frame came", &mut word);
            (OK, n)
        }
        _ => (WARN, write_lit(b"none", &mut word)),
    };
    if let Some(c) = field(x, y, 298, b"Image sensor", &word[..wn], sensor_color) {
        return Some(c);
    }
    if line(x, y, 318, b"derived from the camera_capture() result", FAINT).is_some() {
        return Some(FAINT);
    }

    if center_line(x, y, 360, b"A running camera service is not", MUTED).is_some() {
        return Some(MUTED);
    }
    if center_line(x, y, 388, b"an image sensor. No MIPI-CSI-2", MUTED).is_some() {
        return Some(MUTED);
    }
    center_line(x, y, 416, b"sensor is bound in this platform.", MUTED)
}

#[inline(never)]
fn viewfinder_card(x: usize, y: usize, app: &CameraPreview) -> Option<u32> {
    if x >= VX0 && x < VX1 && y >= VY0 && y < VY1 {
        if y == VY0 || y == VY1 - 1 || x == VX0 || x == VX1 - 1 {
            return Some(EDGE);
        }

        if let Capture::Got { width, height, .. } = app.capture {
            let fw = width as usize;
            let fh = height as usize;
            let sx = (x - VX0) * fw / (VX1 - VX0);
            let sy = (y - VY0) * fh / (VY1 - VY0);
            let off = (sy * fw + sx) * 4;
            let buf = unsafe { &*core::ptr::addr_of!(FRAME_BUF) };
            if off + 3 < buf.len() {
                let r = buf[off] as u32;
                let g = buf[off + 1] as u32;
                let b = buf[off + 2] as u32;
                return Some(0xFF00_0000 | (r << 16) | (g << 8) | b);
            }
            return Some(BG);
        }

        let w = (VX1 - VX0) as i32;
        let h = (VY1 - VY0) as i32;
        let rx = x as i32 - VX0 as i32;
        let ry = y as i32 - VY0 as i32;
        let cx = w / 2;
        let cy = h / 2;

        // Viewfinder reticle brackets
        let in_box = (rx - cx).abs() <= 60 && (ry - cy).abs() <= 44;
        if in_box && (rx - cx).abs() >= 54 && (ry - cy).abs() >= 38 {
            return Some(EDGE_MUTED);
        }
        if in_box && (rx - cx).abs() >= 38 && (ry - cy).abs() >= 54 {
            return Some(EDGE_MUTED);
        }
        let along = (rx - ry).rem_euclid(64);
        if along < 2 {
            return Some(EDGE_MUTED);
        }
        return Some(CARD_INNER);
    }

    if (856..952).contains(&y) && (30..690).contains(&x) {
        if let Capture::Got { width, height, scene, bytes, frame_id } = app.capture {
            let mut v = [0u8; 48];
            let n = write_lit(b"real frame ", &mut v);
            let n = n + fmt_u64(width as u64, &mut v[n..]);
            let n = n + write_lit(b"x", &mut v[n..]);
            let n = n + fmt_u64(height as u64, &mut v[n..]);
            let n = n + write_lit(b", scene ", &mut v[n..]);
            let n = n + write_lit(scene_word(scene), &mut v[n..]);
            let n = n + write_lit(b", id ", &mut v[n..]);
            let n = n + fmt_u64(frame_id as u64, &mut v[n..]);
            let n = n + write_lit(b", ", &mut v[n..]);
            let n = n + fmt_u64(bytes as u64, &mut v[n..]);
            let n = n + write_lit(b" bytes", &mut v[n..]);
            if line(x, y, 874, &v[..n], OK).is_some() {
                return Some(OK);
            }
        } else {
            if center_line(x, y, 874, b"NO SIGNAL - no image sensor is bound", WARN).is_some() {
                return Some(WARN);
            }
            let (reason, rn) = capture_reason(&app.capture);
            if center_line(x, y, 906, &reason[..rn], MUTED).is_some() {
                return Some(MUTED);
            }
            if center_line(x, y, 938, b"camera_capture() returned no pixels", FAINT).is_some() {
                return Some(FAINT);
            }
        }
    }
    None
}

#[inline(never)]
fn controls_card(x: usize, y: usize, app: &CameraPreview) -> Option<u32> {
    for idx in 0..3usize {
        let bx = BTN_X[idx];
        if in_rounded_rect(x, y, bx, BTN_Y, bx + BTN_W, BTN_Y + BTN_H, 8) {
            let enabled = control_enabled(app, idx);
            if on_rounded_rect_border(x, y, bx, BTN_Y, bx + BTN_W, BTN_Y + BTN_H, 8) {
                return Some(if enabled { ACCENT } else { EDGE_MUTED });
            }
            let label = button_label(idx);
            let tx = bx + (BTN_W - font::text_width(label, 2)) / 2;
            if font::text_at(x, y, tx, BTN_Y + 24, 2, label) {
                return Some(if enabled { TEXT } else { FAINT });
            }
            return Some(if enabled { CARD_RAISED } else { RECESSED });
        }
    }

    for idx in 0..3usize {
        let bx = BTN_X[idx];
        if x >= bx
            && x < bx + BTN_W
            && y >= BTN_Y + BTN_H + 4
            && y < BTN_Y + BTN_H + 36
        {
            if control_reason(app, idx).is_empty() {
                return Some(CARD);
            }
            if font::text_at(x, y, bx, BTN_Y + BTN_H + 6, 2, b"unavailable") {
                return Some(WARN);
            }
            if font::text_at(x, y, bx, BTN_Y + BTN_H + 22, 2, b"see status") {
                return Some(FAINT);
            }
            return Some(CARD);
        }
    }

    if in_rounded_rect(x, y, 44, REPROBE_Y, 676, REPROBE_Y + REPROBE_H, 8) {
        if on_rounded_rect_border(x, y, 44, REPROBE_Y, 676, REPROBE_Y + REPROBE_H, 8) {
            return Some(EDGE);
        }
        let label = b"RE-PROBE CAMERA";
        let tx = 44 + (632 - font::text_width(label, 2)) / 2;
        if font::text_at(x, y, tx, REPROBE_Y + 20, 2, label) {
            return Some(TEXT);
        }
        return Some(CARD_RAISED);
    }

    line(x, y, 1236, b"Every control is drawn recessed unless the", MUTED).or_else(|| {
        if font::text_at(x, y, 44, 1262, 2, b"hardware actually answered a call.") {
            Some(MUTED)
        } else {
            None
        }
    })
}

// -----------------------------------------------------------------------------
// Entry point
// -----------------------------------------------------------------------------

fn status(app: &mut CameraPreview, text: &[u8], color: u32) {
    let n = text.len().min(app.status.len());
    app.status[..n].copy_from_slice(&text[..n]);
    app.status_len = n;
    app.status_color = color;
}

fn probe_all(app: &mut CameraPreview) {
    app.capture = probe_capture();
    app.service_state = read_camera_service();
    app.service_heartbeat = read_camera_heartbeat();
    app.autofocus = read_autofocus();
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[CAMERA PREVIEW (Ring 3)] Camera surface inspector starting.").ok();

    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => s,
        Err(_) => {
            std::log(0, "[CAMERA PREVIEW] No display surface available; nothing to draw.").ok();
            std::exit_with_code(1);
        }
    };

    let mut app = CameraPreview {
        capture: Capture::Empty,
        service_state: None,
        service_heartbeat: None,
        autofocus: None,
        flash: None,
        status: [0; 64],
        status_len: 0,
        status_color: MUTED,
    };

    probe_all(&mut app);

    let (status_text, status_color): (&[u8], u32) = match app.capture {
        Capture::Got { .. } => {
            std::log(0, "[CAMERA PREVIEW] camera_capture() returned a real frame.").ok();
            (b"live frame received; scanout blitting pixels", OK)
        }
        Capture::NoSensor(_) => {
            std::log(0, "[CAMERA PREVIEW] camera_capture() reports DeviceError: no sensor is bound.").ok();
            (b"no image sensor bound; viewfinder shows no-signal pattern", WARN)
        }
        Capture::Denied(_) => {
            std::log(0, "[CAMERA PREVIEW] camera_capture() refused: capture capability not held.").ok();
            (b"camera capture refused; viewfinder shows no-signal pattern", WARN)
        }
        Capture::TooSmall(_) => {
            std::log(0, "[CAMERA PREVIEW] camera_capture() reports frame larger than app buffer.").ok();
            (b"frame larger than app buffer; increase allocation", WARN)
        }
        Capture::Empty => {
            std::log(0, "[CAMERA PREVIEW] camera_capture() succeeded but returned 0 bytes.").ok();
            (b"camera returned zero-byte frame", WARN)
        }
        Capture::Malformed => {
            std::log(0, "[CAMERA PREVIEW] camera_capture() metadata had zero width or height.").ok();
            (b"camera metadata had zero dimensions", WARN)
        }
    };
    status(&mut app, status_text, status_color);

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[CAMERA PREVIEW] Initial frame presented to hardware scanout.").ok();

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
                        std::log(0, "[CAMERA PREVIEW] Navigation gesture. Exiting to launcher.").ok();
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        let (ix, iy) = (input.x as usize, input.y as usize);
                        let tapped_back = (24..100).contains(&ix) && (28..68).contains(&iy);
                        if tapped_back || iy >= 1400 {
                            let _ = std::surface_destroy(6, surface);
                            std::exit_with_code(0);
                        }

                        let in_flash = (BTN_X[FLASH]..BTN_X[FLASH] + BTN_W).contains(&ix)
                            && (BTN_Y..BTN_Y + BTN_H).contains(&iy);
                        if in_flash {
                            app.flash = toggle_flash();
                            std::log(0, "[CAMERA PREVIEW] Flash toggled.").ok();
                            status(&mut app, b"flash toggle invoked", ACCENT);
                            render(surface, &app);
                            let _ = std::surface_flip(6);
                            continue;
                        }

                        let in_reprobe = (44..676).contains(&ix)
                            && (REPROBE_Y..REPROBE_Y + REPROBE_H).contains(&iy);
                        if in_reprobe {
                            probe_all(&mut app);
                            std::log(0, "[CAMERA PREVIEW] Re-probed camera service and sensor state.").ok();
                            status(&mut app, b"re-probed camera service and sensor state", ACCENT);
                            render(surface, &app);
                            let _ = std::surface_flip(6);
                            continue;
                        }

                        probe_all(&mut app);
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
    std::log(0, "[PANIC] Camera preview app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
