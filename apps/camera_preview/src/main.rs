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
///
/// The HAL copies a whole frame or fails, so the buffer size is a real limit and
/// is stated on screen rather than hidden.
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
    /// The kernel returned this many bytes of real pixels.
    Got { bytes: usize, width: u32, height: u32, scene: u32, frame_id: u32 },
    /// The call was refused by the capability check.
    Denied(i64),
    /// The call reached the HAL and the HAL has no sensor.
    NoSensor(i64),
    /// The frame did not fit in this app's buffer.
    TooSmall(i64),
    /// The call succeeded but reported a zero-length frame.
    Empty,
    /// The call succeeded but the metadata was not filled in.
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

    fn color(&self) -> u32 {
        match self {
            Capture::Got { .. } => OK,
            _ => WARN,
        }
    }
}

struct CameraPreview {
    capture: Capture,
    service_state: Option<(u8, u32, bool)>,
    /// Heartbeat age in 1 kHz ticks, and the recorded failure count.
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

/// One real capture attempt.
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
            // The HAL copies whole frames, so a short read is a metadata fault,
            // not a partial frame.
            let needed = meta[0] as usize * meta[1] as usize * 4;
            if bytes < needed || needed > FRAME_BUF_BYTES {
                return Capture::TooSmall(0);
            }
            Capture::Got { bytes, width: meta[0], height: meta[1], scene: meta[2], frame_id: meta[3] }
        }
// WHY these two codes and not their names: `SyscallResult` is a repr(i64)
        // enum, so -3 is `InvalidArguments` -- which `sys_camera_capture` returns
        // when the frame does not fit the caller's buffer -- and -12 is
        // `DeviceError`, which it returns when `capture_frame()` has no sensor
        // behind it. Every other negative value, -1 `PermissionDenied` included,
        // means the call never reached the HAL.
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

/// Parses the camera service's own line out of the `service_list` snapshot.
///
/// The snapshot format is `id|abi|state|restart|critical|generation|age|failures`
/// per service, so heartbeat age and failure count come from the kernel rather
/// than from anything this app infers.
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

/// Autofocus actuator state, straight from the camera HAL.
fn read_autofocus() -> Option<bool> {
    std::camera_autofocus_state().ok()
}

/// Flash torch state, straight from the camera HAL. Returns the new state.
fn toggle_flash() -> Option<bool> {
    std::camera_toggle_flash().ok()
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

fn scene_word(scene: u32) -> &'static [u8] {
    // SceneType from the HAL, in the order the kernel maps it.
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

/// Right-aligned reason text for a capture outcome, including the status code.
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
// Palette and layout
// -----------------------------------------------------------------------------

const BG: u32 = 0xFF070B12;
const CARD: u32 = 0xFF111B2B;
const CARD_RAISED: u32 = 0xFF17243A;
/// A control that cannot work: recessed one step below CARD_RAISED.
const RECESSED: u32 = 0xFF0D1522;
const EDGE: u32 = 0xFF26354D;
const EDGE_MUTED: u32 = 0xFF1B2536;
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

const CARDS: [Card; 3] = [
    Card { x0: 30, y0: 112, x1: 690, y1: 452, caption: b"CAMERA SERVICE (SERVICE 8)" },
    Card { x0: 30, y0: 472, x1: 690, y1: 952, caption: b"VIEWFINDER SURFACE" },
    Card { x0: 30, y0: 972, x1: 690, y1: 1312, caption: b"CONTROLS" },
];

/// Viewfinder rectangle inside its card.
const VX0: usize = 60;
const VX1: usize = 660;
const VY0: usize = 520;
const VY1: usize = 856;

/// Control geometry.
const BTN_Y: usize = 1032;
const BTN_H: usize = 64;
const BTN_W: usize = 196;
const BTN_X: [usize; 3] = [44, 262, 480];
const REPROBE_Y: usize = 1148;
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

/// Whether a control can actually do anything, based on the last real call.
fn control_enabled(app: &CameraPreview, idx: usize) -> bool {
    match idx {
        SHUTTER => matches!(app.capture, Capture::Got { .. }),
        AF_LOCK => app.autofocus.is_some(),
        _ => app.flash.is_some(),
    }
}

/// The one-line reason a control is unavailable.
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
    if y < 96 {
        if y >= 94 {
            return EDGE;
        }
        if (24..100).contains(&x) && (28..68).contains(&y) {
            let border = y == 28 || y == 67 || x == 24 || x == 99;
            let chev = (x as isize - 62).abs() + (y as isize - 48).abs() <= 11 && x <= 62;
            return if border || chev { TEXT } else { CARD_RAISED };
        }
        if font::text_at(x, y, 116, 32, 3, b"Camera Preview") {
            return TEXT;
        }
        return CARD;
    }

    if (1312..1380).contains(&y) && (30..690).contains(&x) {
        if y == 1312 || y == 1379 || x == 30 || x == 689 {
            return EDGE;
        }
        if font::text_at(x, y, 44, 1334, 2, &app.status[..app.status_len]) {
            return app.status_color;
        }
        return CARD;
    }

    for card in CARDS.iter() {
        if y >= card.y0 && y < card.y1 && x >= card.x0 && x < card.x1 {
            if y == card.y0 || y == card.y1 - 1 || x == card.x0 || x == card.x1 - 1 {
                return EDGE;
            }
            if font::text_at(x, y, card.x0 + 14, card.y0 + 12, 2, card.caption) {
                return ACCENT;
            }
            return CARD;
        }
    }

    if (112..452).contains(&y) && (30..690).contains(&x) {
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
    if (972..1312).contains(&y) && (30..690).contains(&x) {
        if let Some(c) = controls_card(x, y, app) {
            return c;
        }
        return CARD;
    }

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
    if let Some(c) = field(x, y, 160, b"Service state", &word[..wn], color) {
        return Some(c);
    }
    if line(x, y, 180, b"service_query(8)", FAINT).is_some() {
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
    if let Some(c) = field(x, y, 216, b"Heartbeat", &hb_text.0[..hb_text.1], hb_color) {
        return Some(c);
    }
if line(x, y, 236, b"service_list() heartbeat age in 1 kHz ticks / failures", FAINT).is_some() {
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
    if let Some(c) = field(x, y, 272, b"Image sensor", &word[..wn], sensor_color) {
        return Some(c);
    }
    if line(x, y, 292, b"derived from the camera_capture() result", FAINT).is_some() {
        return Some(FAINT);
    }

    if center_line(x, y, 340, b"A running camera service is not", MUTED).is_some() {
        return Some(MUTED);
    }
    if center_line(x, y, 368, b"an image sensor. No MIPI-CSI-2", MUTED).is_some() {
        return Some(MUTED);
    }
    center_line(x, y, 396, b"sensor is bound in this platform.", MUTED)
}

#[inline(never)]
fn viewfinder_card(x: usize, y: usize, app: &CameraPreview) -> Option<u32> {
    // The surface itself.
    if x >= VX0 && x < VX1 && y >= VY0 && y < VY1 {
        if y == VY0 || y == VY1 - 1 || x == VX0 || x == VX1 - 1 {
            return Some(EDGE);
        }

        if let Capture::Got { width, height, .. } = app.capture {
            // Real pixels, nearest-neighbour scaled into the surface.
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

        // No frame. An explicitly empty surface with a no-signal mark: a diagonal
        // hatch and a crossed box, so it cannot be mistaken for a dark picture.
        let w = (VX1 - VX0) as i32;
        let h = (VY1 - VY0) as i32;
        let rx = x as i32 - VX0 as i32;
        let ry = y as i32 - VY0 as i32;
        let cx = w / 2;
        let cy = h / 2;
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
        return Some(BG);
    }

    // The caption under the surface, from the call that produced this state.
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
        if x >= bx && x < bx + BTN_W && y >= BTN_Y && y < BTN_Y + BTN_H {
            let enabled = control_enabled(app, idx);
            if y == BTN_Y || y == BTN_Y + BTN_H - 1 || x == bx || x == bx + BTN_W - 1 {
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

    // Reason under each disabled control, on the same row as the label it applies to.
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

    if x >= 44 && x < 676 && y >= REPROBE_Y && y < REPROBE_Y + REPROBE_H {
        if y == REPROBE_Y || y == REPROBE_Y + REPROBE_H - 1 || x == 44 || x == 675 {
            return Some(EDGE);
        }
        let label = b"RE-PROBE CAMERA";
        let tx = 44 + (632 - font::text_width(label, 2)) / 2;
        if font::text_at(x, y, tx, REPROBE_Y + 20, 2, label) {
            return Some(TEXT);
        }
        return Some(CARD_RAISED);
    }

    line(x, y, 1240, b"Every control is drawn recessed unless the", MUTED).or_else(|| {
        if font::text_at(x, y, 44, 1266, 2, b"hardware actually answered a call.") {
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
    // The flash torch is deliberately not touched here. The SDK exposes no
    // read-only flash getter, so re-probing must not change the torch behind the
    // user's back; the FLASH control is the only thing that may toggle it.
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
    // The SDK exposes a flash *toggle* and no read-only getter, so the only way to
    // learn the torch's state is to flip it once. That is a real hardware change,
    // and it is stated rather than dressed up as a read: the initial state below is
    // whatever this single toggle reported, and the FLASH control is the only other
    // thing that may touch it.
    app.flash = std::camera_toggle_flash().ok();

    match app.capture {
        Capture::Got { .. } => {
            std::log(0, "[CAMERA PREVIEW] A real frame arrived; blitting the returned pixels.").ok();
        }
        Capture::Denied(e) => {
            std::log(
                0,
                "[CAMERA PREVIEW] camera_capture refused: this app holds no camera control object.",
            )
            .ok();
            let _ = e;
        }
        Capture::NoSensor(e) => {
            std::log(
                0,
                "[CAMERA PREVIEW] Camera HAL reports no sensor backend; the surface is empty.",
            )
            .ok();
            let _ = e;
        }
        _ => {
            std::log(0, "[CAMERA PREVIEW] Camera returned no pixels; reporting an empty surface.").ok();
        }
    }

    let (reason, rn) = capture_reason(&app.capture);
    let reason_color = app.capture.color();
    status(&mut app, &reason[..rn], reason_color);

    render(surface, &app);
    let _ = std::surface_flip(6);
    std::log(0, "[CAMERA PREVIEW] Initial frame presented to hardware scanout.").ok();

    let mut refresh = false;
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
                    if input.kind != 1 {
                        continue;
                    }
                    let x = input.x as usize;
                    let y = input.y as usize;

                    if ((24..100).contains(&x) && (28..68).contains(&y)) || y >= 1400 {
                        let _ = std::surface_destroy(6, surface);
                        std::exit_with_code(0);
                    }

                    if x >= 44 && x < 676 && (REPROBE_Y..REPROBE_Y + REPROBE_H).contains(&y) {
                        probe_all(&mut app);
                        status(&mut app, b"re-probed: capture, service record, heartbeat", ACCENT);
                        refresh = true;
                        continue;
                    }

                    let mut hit: Option<usize> = None;
                    for idx in 0..3usize {
                        if x >= BTN_X[idx]
                            && x < BTN_X[idx] + BTN_W
                            && (BTN_Y..BTN_Y + BTN_H).contains(&y)
                        {
                            hit = Some(idx);
                        }
                    }
                    if hit.is_none() {
                        continue;
                    }
                    let idx = hit.unwrap();
                    if idx >= 3 {
                        continue;
                    }

                    if !control_enabled(&app, idx) {
                        let reason = control_reason(&app, idx);
                        status(&mut app, reason, WARN);
                        std::log(0, "[CAMERA PREVIEW] Control refused: no hardware answered.").ok();
                        refresh = true;
                        continue;
                    }

                    match idx {
                        SHUTTER => {
                            app.capture = probe_capture();
                            let (reason, rn) = capture_reason(&app.capture);
                            let color = app.capture.color();
                            status(&mut app, &reason[..rn], color);
                        }
                        AF_LOCK => {
                            app.autofocus = read_autofocus();
                            let on = app.autofocus.unwrap_or(false);
                            status(
                                &mut app,
                                if on { b"autofocus reported LOCKED" } else { b"autofocus reported idle" },
                                OK,
                            );
                        }
                        FLASH => {
                            app.flash = toggle_flash();
                            let (text, color) = match app.flash {
                                Some(true) => (&b"flash torch reported ON"[..], OK),
                                Some(false) => (&b"flash torch reported OFF"[..], OK),
                                None => (&b"flash torch refused the request"[..], WARN),
                            };
                            status(&mut app, text, color);
                        }
                        _ => {}
                    }
                    refresh = true;
                }
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
    std::log(0, "[PANIC] Camera preview app encountered an unrecoverable error!").ok();
    std::exit_with_code(1);
}
