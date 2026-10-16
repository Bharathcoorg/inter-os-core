//! System Space: device, network, personal, apps, power, and security settings.
//!
//! Pixel-perfect reference implementation for Screen 6 matching the Sovereign
//! InterOS Core reference architecture (media_1791278018153.jpg).

#![allow(unused_imports, dead_code)]

use crate::screens::lock::LOCK_DATE_TEXT_LEN;
use crate::{test_text_aa, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::*;

const THEME: Theme = Theme::LIGHT;

/// Maximum value buffer capacity.
pub(super) const VALUE_MAX: usize = 48;

/// Pinned Power entry hitbox (matches main.rs touch handler: x in 40..345, y in 590..710).
pub const POWER_ENTRY: Rect = Rect::new(40, 590, 305, 120);

// -----------------------------------------------------------------------------
// Shared Primitives & Helpers (consumed by live_widgets and power)
// -----------------------------------------------------------------------------

/// Whether `(x, y)` falls inside `rect` with corners rounded to `radius`.
pub(super) fn round_contains(rect: &Rect, radius: usize, x: usize, y: usize) -> bool {
    if !rect.contains(x, y) {
        return false;
    }
    let r = radius.min(rect.w / 2).min(rect.h / 2);
    if r == 0 {
        return true;
    }
    let left = rect.x + r;
    let right = rect.x + rect.w - r;
    let top = rect.y + r;
    let bottom = rect.y + rect.h - r;
    let cx = if x < left {
        left
    } else if x >= right {
        right
    } else {
        x
    };
    let cy = if y < top {
        top
    } else if y >= bottom {
        bottom
    } else {
        y
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    (dx * dx + dy * dy) as usize <= r * r
}

/// Colours pixel if it falls on `run` left-aligned inside `rect`.
pub(super) fn run_left(
    x: usize,
    y: usize,
    rect: &Rect,
    scale: usize,
    run: &[u8],
    color: u32,
) -> Option<u32> {
    let h = text::line_height(scale);
    let y0 = rect.y + rect.h.saturating_sub(h) / 2;
    if y < y0 || y >= y0 + h || x < rect.x {
        return None;
    }
    let w = text::measure(run, scale);
    if x >= rect.x + w {
        return None;
    }
    test_text_aa(x, y, rect.x, y0, scale, run, color, 0xFFFFFFFF)
}

/// Colours pixel if it falls on `run` right-aligned inside `rect`.
pub(super) fn run_right(
    x: usize,
    y: usize,
    rect: &Rect,
    scale: usize,
    run: &[u8],
    color: u32,
) -> Option<u32> {
    let h = text::line_height(scale);
    let y0 = rect.y + rect.h.saturating_sub(h) / 2;
    if y < y0 || y >= y0 + h {
        return None;
    }
    let w = text::measure(run, scale);
    if w > rect.w {
        return None;
    }
    let shifted = Rect::new(rect.x + rect.w - w, rect.y, w, rect.h);
    run_left(x, y, &shifted, scale, run, color)
}

/// Colours pixel if it falls on `run` horizontally centred inside `rect`.
pub(super) fn run_center(
    x: usize,
    y: usize,
    rect: &Rect,
    scale: usize,
    run: &[u8],
    color: u32,
) -> Option<u32> {
    let h = text::line_height(scale);
    let y0 = rect.y + rect.h.saturating_sub(h) / 2;
    if y < y0 || y >= y0 + h {
        return None;
    }
    let w = text::measure(run, scale);
    if w > rect.w {
        return None;
    }
    let placed = Rect::new(rect.x + text::center_x(run, scale, rect.w), rect.y, w, rect.h);
    run_left(x, y, &placed, scale, run, color)
}

/// A settings value or absence reason.
pub(super) struct Reading {
    bytes: [u8; VALUE_MAX],
    len: usize,
    available: bool,
}

impl Reading {
    pub(super) fn of(run: &[u8]) -> Reading {
        let mut bytes = [0u8; VALUE_MAX];
        let len = run.len().min(VALUE_MAX);
        bytes[..len].copy_from_slice(&run[..len]);
        Reading { bytes, len, available: true }
    }

    pub(super) fn unavailable(reason: &[u8]) -> Reading {
        let mut reading = Reading::of(reason);
        reading.available = false;
        reading
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub(super) fn skin(&self, theme: &Theme) -> (u32, u32) {
        if self.available {
            (theme.surface, theme.text)
        } else {
            (theme.surface_sunken, theme.text_muted)
        }
    }
}

/// A rounded card cell with label and value.
pub(super) struct Cell<'a> {
    pub rect: Rect,
    pub corner: usize,
    pub label: &'a [u8],
    pub value: &'a [u8],
    pub fill: u32,
    pub value_ink: u32,
    pub scale: usize,
}

impl<'a> Cell<'a> {
    pub(super) fn reading(
        rect: Rect,
        corner: usize,
        label: &'a [u8],
        value: &'a Reading,
        theme: &Theme,
    ) -> Cell<'a> {
        Cell::scaled(rect, corner, label, value, 2, theme)
    }

    pub(super) fn scaled(
        rect: Rect,
        corner: usize,
        label: &'a [u8],
        value: &'a Reading,
        scale: usize,
        theme: &Theme,
    ) -> Cell<'a> {
        let (fill, value_ink) = value.skin(theme);
        Cell { rect, corner, label, value: value.as_slice(), fill, value_ink, scale }
    }

    pub(super) fn contains(&self, x: usize, y: usize) -> bool {
        round_contains(&self.rect, self.corner, x, y)
    }
}

pub(super) fn cell_pixel(x: usize, y: usize, theme: &Theme, cell: &Cell) -> u32 {
    if !cell.rect.contains(x, y) || !cell.contains(x, y) {
        return theme.bg;
    }
    let inner = cell.rect.inset(1);
    if !round_contains(&inner, cell.corner.saturating_sub(1), x, y) {
        return theme.border;
    }

    let pad = cell.rect.inset(space::MD);
    let h = text::line_height(cell.scale);
    let text_y0 = pad.y + pad.h.saturating_sub(h) / 2;
    if y < text_y0 || y >= text_y0 + h {
        return cell.fill;
    }

    let value_w = text::measure(cell.value, cell.scale).min(pad.w * 50 / 100) + space::SM;
    let value_col = Rect::new(pad.x + pad.w - value_w, pad.y, value_w, pad.h);
    let label_col = Rect::new(pad.x, pad.y, pad.w - value_w - space::SM, pad.h);

    if x >= value_col.x {
        let (run, len) = text::ellipsize(cell.value, cell.scale, value_col.w);
        if let Some(c) = run_right(x, y, &value_col, cell.scale, &run[..len], cell.value_ink) {
            return c;
        }
    } else if x < label_col.x + label_col.w {
        let (run, len) = text::ellipsize(cell.label, cell.scale, label_col.w);
        if let Some(c) = run_left(x, y, &label_col, cell.scale, &run[..len], theme.text) {
            return c;
        }
    }
    cell.fill
}

pub(super) fn push(buf: &mut [u8; VALUE_MAX], at: &mut usize, run: &[u8]) {
    for b in run {
        if *at >= VALUE_MAX {
            return;
        }
        buf[*at] = *b;
        *at += 1;
    }
}

pub(super) fn decimal(n: usize) -> ([u8; VALUE_MAX], usize) {
    let mut out = [b' '; VALUE_MAX];
    if n == 0 {
        out[0] = b'0';
        return (out, 1);
    }
    let mut v = n;
    let mut at = 0usize;
    while v > 0 && at < VALUE_MAX {
        out[at] = b'0' + (v % 10) as u8;
        at += 1;
        v /= 10;
    }
    out[..at].reverse();
    (out, at)
}

pub(super) fn darken(base: u32, percent: u32) -> u32 {
    let keep = 256 - (percent.min(100) * 256) / 100;
    let mut out = ((base >> 24) & 255) << 24;
    for shift in [16u32, 8, 0] {
        let channel = (((base >> shift) & 255) * keep) >> 8;
        out |= channel.min(255) << shift;
    }
    out
}

pub(super) fn counted(n: usize, suffix: &[u8]) -> ([u8; VALUE_MAX], usize) {
    let (mut buf, mut at) = decimal(n);
    push(&mut buf, &mut at, suffix);
    (buf, at)
}

// -----------------------------------------------------------------------------
// Vector Icon Drawers
// -----------------------------------------------------------------------------

#[inline]
fn draw_device_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    if dx >= -7 && dx <= 7 && dy >= -11 && dy <= 11 {
        let is_border = dx <= -6 || dx >= 6 || dy <= -10 || dy >= 10;
        let is_speaker = dy == -8 && dx >= -2 && dx <= 2;
        let is_home = dy == 8 && dx >= -3 && dx <= 3;
        if is_border || is_speaker || is_home {
            return Some(tint);
        }
    }
    None
}

#[inline]
fn draw_network_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    if dy <= 3 {
        if d2 >= 169 && d2 <= 289 && dx.abs() <= 15 {
            return Some(tint);
        }
        if d2 >= 49 && d2 <= 100 && dx.abs() <= 9 {
            return Some(tint);
        }
    }
    if d2 <= 9 && dy >= 0 {
        return Some(tint);
    }
    None
}

#[inline]
fn draw_personal_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    if d2 >= 121 && d2 <= 169 {
        return Some(tint);
    }
    let h_dy = dy + 4;
    if dx * dx + h_dy * h_dy <= 16 {
        return Some(tint);
    }
    if dy >= 3 && dy <= 10 && d2 < 121 {
        let shoulder_w = (11 - dy) * 2;
        if dx.abs() <= shoulder_w {
            return Some(tint);
        }
    }
    None
}

#[inline]
fn draw_apps_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let in_col1 = dx >= -9 && dx <= -3;
    let in_col2 = dx >= 3 && dx <= 9;
    let in_row1 = dy >= -9 && dy <= -3;
    let in_row2 = dy >= 3 && dy <= 9;
    if (in_col1 || in_col2) && (in_row1 || in_row2) {
        return Some(tint);
    }
    None
}

#[inline]
fn draw_battery_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    if dx >= -11 && dx <= 8 && dy >= -6 && dy <= 6 {
        if dx <= -10 || dx >= 7 || dy <= -5 || dy >= 5 {
            return Some(tint);
        }
        // WHY a centred dash and not `if dx <= 2 { return Some(tint) }`: that arm
        // filled the left 60% of every shell with no reading behind it, so the
        // status bar asserted a charge level this shell never measured - on the
        // one screen that also claims to be a truthful summary of the device.
        // No fuel gauge is bound, so the glyph says "not known" the same way the
        // home surface does.
        if dy == 0 && dx >= -4 && dx <= 3 {
            return Some(tint);
        }
    }
    if dx >= 8 && dx <= 11 && dy >= -3 && dy <= 3 {
        return Some(tint);
    }
    None
}

#[inline]
fn draw_security_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    if dy >= -10 && dy <= 11 && dx.abs() <= 9 {
        let max_dx = if dy <= 0 {
            9
        } else {
            9 - (dy * 9) / 12
        };
        if dx.abs() <= max_dx {
            let is_edge = dx.abs() >= max_dx - 2 || dy <= -8 || dy >= 9;
            let is_keyhole = (dx.abs() <= 2 && dy >= -2 && dy <= 4) || (dx * dx + (dy + 3) * (dy + 3) <= 9);
            if is_edge || is_keyhole {
                return Some(tint);
            }
        }
    }
    None
}

#[inline]
fn draw_gear_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    if d2 <= 144 && d2 >= 25 {
        let is_spoke = dx.abs() <= 3 || dy.abs() <= 3 || (dx - dy).abs() <= 3 || (dx + dy).abs() <= 3;
        let in_ring = d2 >= 25 && d2 <= 81;
        if in_ring || (is_spoke && d2 <= 144) {
            return Some(tint);
        }
    }
    None
}

// -----------------------------------------------------------------------------
// 3D Glass Ambient Sphere
// -----------------------------------------------------------------------------

/// A small sine table in 64ths, indexed by a 64-step turn.
///
/// WHY a table and not a series: this is `no_std` integer-only, and the orb
/// needs a rotating light vector every frame. A fixed-point rotation needs both
/// sin and cos, and a table makes both a lookup. 64 steps over a full turn is
/// 5.6 degrees, which at the orb's 52px radius is under half a pixel of light
/// travel per step -- finer than the shading can resolve.
const ORB_TURN_STEPS: u32 = 64;
const ORB_TRIG_SCALE: i32 = 64;

const ORB_SIN: [i32; ORB_TURN_STEPS as usize] = [
    0, 6, 12, 19, 24, 30, 36, 41, 45, 49, 53, 56, 59, 61, 63, 64, 64, 64, 63, 61, 59, 56, 53, 49,
    45, 41, 36, 30, 24, 19, 12, 6, 0, -6, -12, -19, -24, -30, -36, -41, -45, -49, -53, -56, -59,
    -61, -63, -64, -64, -64, -63, -61, -59, -56, -53, -49, -45, -41, -36, -30, -24, -19, -12, -6,
];

#[inline]
fn orb_sin(step: u32) -> i32 {
    ORB_SIN[(step % ORB_TURN_STEPS) as usize]
}

#[inline]
fn orb_cos(step: u32) -> i32 {
    // A quarter turn ahead, from the same table.
    ORB_SIN[((step + ORB_TURN_STEPS / 4) % ORB_TURN_STEPS) as usize]
}

/// The orb's light direction, rotated to `step` of a turn.
///
/// WHY the rotation is in the plane of the surface and not in 3D: the sphere's
/// normal at a pixel is derived from (dx, dy, z) by the caller, so rotating the
/// light's (x, y) while leaving z alone sweeps the highlight around the body.
/// That is the motion that reads as a lit ball turning rather than as the
/// shading changing.
#[inline]
fn orb_light(step: u32) -> (i32, i32, i32) {
    // A light high and to the left, the same resting angle the static sphere
    // used, expressed as a unit-ish vector in the table's 64ths.
    let (lx, ly, lz) = (-3i32, -4i32, 4i32);
    let s = orb_sin(step);
    let c = orb_cos(step);
    (
        (lx * c - ly * s) / ORB_TRIG_SCALE,
        (lx * s + ly * c) / ORB_TRIG_SCALE,
        lz,
    )
}

/// The orb's radius for this frame, breathing.
///
/// WHY a breath and not a bounce: a radius that grows and shrink on a slow
/// cycle reads as a body under pressure, and it is the one dimension that can
/// change without moving anything on the surface, so nothing has to reflow. The
/// amplitude is two rows around a 52px base, which is 4% -- large enough to see
/// as life and small enough that the drop shadow below stays put.
#[inline]
fn orb_radius(base: usize, ticks: u64) -> usize {
    let phase = (ticks / 24) as u32; // ~ a few frames per step at 100Hz
    let delta = orb_sin(phase) / 38; // +/- 2 rows
    (base as i32 + delta).max(1) as usize
}

/// The back control: a 48x48 soft pill with a 2px chevron.
///
/// WHY this exists on this surface: the Settings app opens the same screen with
/// a back pill, and so does the reference, but the shell's copy had none -- so a
/// user who arrived from the home surface's search bar had only a gesture to get
/// back with. The shape matches the Settings app's so the two agree wherever a
/// user would compare them.
pub const BACK_X: usize = 48;
/// WHY 56 and not the 24 it was: the status rule is at row 48, and a control
/// starting at 24 sits underneath it -- the top of the pill was drawn over the
/// rule and the chevron's top was cut by the surface edge. Eight rows past the
/// rule is the same clearance the reference leaves, and it puts the pill fully
/// inside the content area where every other control on this surface lives.
pub const BACK_Y: usize = 56;
pub const BACK_S: usize = 48;
/// WHY the launcher's own `radius::CONTROL` and not the SDK's: the SDK scale and
/// the launcher's scale carry the same names at different values -- the SDK's
/// `CONTROL` is 12 and the launcher's is 18 -- and this was the only launcher
/// screen reaching across for the SDK one. The result was that this back pill was
/// drawn with visibly tighter corners than every control beside it. One app
/// should resolve its radius against one scale.
pub const BACK_R: usize = radius::CONTROL;

/// True when (`x`, `y`) is inside a rounded rectangle of `r` corner radius.
#[inline]
fn in_round_rect(x: usize, y: usize, rx: usize, ry: usize, rw: usize, rh: usize, r: usize) -> bool {
    if x < rx || y < ry || x >= rx + rw || y >= ry + rh {
        return false;
    }
    // The four corner boxes; a pixel in one is inside only if it is also inside
    // that corner's quarter disc.
    let cx = if x < rx + r {
        rx + r
    } else if x >= rx + rw - r {
        rx + rw - r - 1
    } else {
        return true; // Not in a corner box at all.
    };
    let cy = if y < ry + r {
        ry + r
    } else if y >= ry + rh - r {
        ry + rh - r - 1
    } else {
        return true;
    };
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    (dx * dx + dy * dy) as u32 <= (r * r) as u32
}

/// Squared distance from (`x`,`y`) to the segment (`x1`,`y1`)-(`x2`,`y2`).
///
/// WHY squared and not the distance itself: this is `no_std` integer-only, and
/// every comparison this serves is against a squared radius, so a square root
/// would be computed and then thrown away.
fn dist2_to_seg(x: usize, y: usize, x1: i64, y1: i64, x2: i64, y2: i64) -> i64 {
    let vx = x2 - x1;
    let vy = y2 - y1;
    let wx = (x as i64) - x1;
    let wy = (y as i64) - y1;
    let len2 = vx * vx + vy * vy;
    let t = if len2 == 0 {
        0
    } else {
        let tt = (wx * vx + wy * vy) * 256 / len2;
        tt.clamp(0, 256)
    };
    let px = x1 + vx * t / 256;
    let py = y1 + vy * t / 256;
    let dx = (x as i64) - px;
    let dy = (y as i64) - py;
    dx * dx + dy * dy
}

/// The chevron: two strokes meeting at a point, 2px wide.
#[inline]
fn chevron_ink(x: usize, y: usize, cx: usize, cy: usize, color: u32) -> Option<u32> {
    let (cx, cy) = (cx as i64, cy as i64);
    if dist2_to_seg(x, y, cx + 6, cy - 10, cx - 6, cy) <= 4
        || dist2_to_seg(x, y, cx - 6, cy, cx + 6, cy + 10) <= 4
    {
        Some(color)
    } else {
        None
    }
}

/// The whole back control, as a pixel.
fn back_control_pixel(x: usize, y: usize) -> Option<u32> {
    if !in_round_rect(x, y, BACK_X, BACK_Y, BACK_S, BACK_S, BACK_R) {
        return None;
    }
    if !in_round_rect(x, y, BACK_X + 1, BACK_Y + 1, BACK_S - 2, BACK_S - 2, BACK_R - 1) {
        return Some(TEXT_DARK);
    }
    if let Some(c) = chevron_ink(x, y, BACK_X + BACK_S / 2, BACK_Y + BACK_S / 2, TEXT_DARK) {
        return Some(c);
    }
    Some(0xFFFFFFFF)
}

/// Draws the living orb at (`cx`, `cy`) for this frame.
///
/// WHY `ticks` is a parameter: the whole animation is a function of the frame
/// clock, and passing it keeps the function pure -- the same frame drawn twice
/// gives the same orb, which is what makes a capture reproducible.
fn draw_orb(x: usize, y: usize, cx: usize, cy: usize, base_r: usize, ticks: u64) -> Option<u32> {
    let r = orb_radius(base_r, ticks);
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    let r2 = (r * r) as i32;

    // Contact drop shadow below the orb. Its scale follows the radius so the
    // shadow tightens as the body grows, which is what sells them as one object.
    let shadow_dy = dy - 56;
    let s_rad = (r as i32 * 60) / 52;
    let s_dx = dx;
    let s_dist = s_dx * s_dx * 100 / (s_rad * s_rad) + shadow_dy * shadow_dy * 100 / (10 * 10);
    if s_dist <= 100 && d2 > r2 {
        let alpha = (100 - s_dist) as u32;
        let keep = 256 - (alpha * 65 / 100);
        let r_c = (0xF8 * keep + 0x0F * (256 - keep)) >> 8;
        let g_c = (0xFA * keep + 0x17 * (256 - keep)) >> 8;
        let b_c = (0xFC * keep + 0x2A * (256 - keep)) >> 8;
        return Some(0xFF000000 | (r_c << 16) | (g_c << 8) | b_c);
    }

    if d2 <= r2 {
        let z2 = r2 - d2;
        let mut z = 0i32;
        while (z + 1) * (z + 1) <= z2 {
            z += 1;
        }

        // The light, rotated to this frame's turn.
        let (lx, ly, lz) = orb_light((ticks / 6) as u32);
        // The surface normal at this pixel, pointing out of the sphere.
        let nx = dx;
        let ny = dy;
        let nz = z;
        let dot = (lx * nx + ly * ny + lz * nz) / (r as i32 * 4);
        let diffuse = (dot / 3).max(0).min(6);

        // The specular highlight rides the same rotated direction, offset so it
        // lands on the upper-left of the lit side rather than on the light
        // vector itself.
        let spec_dx = dx + (lx * 5) / 4;
        let spec_dy = dy + (ly * 5) / 4;
        let spec_d2 = spec_dx * spec_dx + spec_dy * spec_dy;
        let spec_r = (r * r) / 36;

        let mut cr = 24 + diffuse * 8;
        let mut cg = 30 + diffuse * 10;
        let mut cb = 42 + diffuse * 14;

        // A rim that reads as the far side of the body, brightening where the
        // surface turns away.
        if z <= 12 {
            let rim = (12 - z) * 3;
            cr += rim;
            cg += rim + 2;
            cb += rim + 5;
        }

        let sr2 = (spec_r * spec_r) as i32;
        if spec_d2 <= sr2 {
            let spec_intensity = (sr2 - spec_d2) as i32;
            cr += spec_intensity * 120 / sr2.max(1);
            cg += spec_intensity * 135 / sr2.max(1);
            cb += spec_intensity * 155 / sr2.max(1);
        }
        // A hot core inside the highlight, so the lit side reads as a specular
        // point rather than a gradient.
        if spec_d2 <= 25 {
            let hot = (25 - spec_d2) as i32;
            cr += hot * 70 / 25;
            cg += hot * 70 / 25;
            cb += hot * 70 / 25;
        }

        let r_clamped = (cr.min(255)) as u32;
        let g_clamped = (cg.min(255)) as u32;
        let b_clamped = (cb.min(255)) as u32;
        return Some(0xFF000000 | (r_clamped << 16) | (g_clamped << 8) | b_clamped);
    }
    None
}

// -----------------------------------------------------------------------------
// Card Geometry
// -----------------------------------------------------------------------------
//
// WHY this stack: the reference screen puts the back control, then the heading,
// then the orb with its subtitle, then the cards -- and it has room for all of
// it because the cards only reach two thirds of the way down. The old stack put
// the heading and the orb in the same 49-row band the back control now needs,
// which is why the control and the title's first letters overlapped, and it
// left 600 rows of dead space below the last card. Every band below is measured
// against the one above it so they cannot drift back into each other.
//
// status rule                 0..48
// back pill                  56..104
// heading                  112..184   (ink 121..169)
// orb                      200..310
// subtitle                 322..344
// cards  row 1             370..490
//        row 2             510..630
//        row 3             650..770
//        full width        770..890
//
// WHY row 4 is 770 and not 790: it is the one full-width card, and it holds the
// same three parts the 120px rows hold -- a 22px icon, a 48px title box and a
// 24px caption box. Typed as a 110px card at y 790, both ends of that stack fell
// outside its own rectangle: the gear icon's centre was placed 16 rows above the
// card's top edge, and the caption's line box ran to row 906, six rows past the
// bottom. A per-pixel painter cannot clip itself, so the card's own gate threw
// those rows away -- the capture shows the gear sliced flat at the top border and
// the caption's descenders missing. Making row 4 the same height as rows 1-3 and
// moving it up one row-3 card height puts all three parts back inside the card,
// which is all the fix this surface can have: 890 leaves 546 rows of air below,
// and the home indicator at 1416 is nowhere near.
//
// WHY one rhythm for all four rows: each row's icon centre, title box and caption
// box used to be typed by hand per card, and they drifted -- row 1's title was
// placed 48 rows ABOVE its card so it never drew at all, row 2 sat 15 rows higher
// in the card than row 3, and row 4 clipped both ends. A painter cannot omit a
// run because its box is outside its parent, so every one of those was a silent
// "this text does not exist". Deriving all three from the card's own origin makes
// the relationship the code and not a number a reader has to trust.
//
// The rhythm, measured against the two rows that did render correctly (rows 2 and
// 3) and kept because it fits a 120px card with air to spare:
//   icon   +26..+48   (a 22px-tall pictogram centred on +37)
//   title  +42..+90   (the 48px line box a scale-2 run needs)
//   caption+92..+116  (the 24px line box a scale-1 run needs)
const CARD_H: usize = 120;

/// Rows from a card's top edge to its pictogram's centre.
const CARD_ICON_DY: usize = 37;
/// Rows from a card's top edge to its title's line box.
const CARD_TITLE_DY: usize = 42;
/// Rows from a card's top edge to its caption's line box.
const CARD_CAPTION_DY: usize = 92;

const CARD_R1_L: Rect = Rect::new(40, 370, 305, CARD_H);
const CARD_R1_R: Rect = Rect::new(375, 370, 305, CARD_H);
const CARD_R2_L: Rect = Rect::new(40, 510, 305, CARD_H);
const CARD_R2_R: Rect = Rect::new(375, 510, 305, CARD_H);
const CARD_R3_L: Rect = Rect::new(40, 650, 305, CARD_H); // POWER_ENTRY
const CARD_R3_R: Rect = Rect::new(375, 650, 305, CARD_H);
const CARD_R4: Rect = Rect::new(40, 770, 640, CARD_H);

/// Rows the orb occupies, and so the band its driver repaints.
const ORB_BAND_Y0: usize = 200;
const ORB_BAND_Y1: usize = 310;

const BG_COLOR: u32 = 0xFFF8FAFC;
const CARD_BG: u32 = 0xFFFFFFFF;
const CARD_BORDER: u32 = 0xFFE2E8F0;
const TEXT_DARK: u32 = 0xFF0F172A;
const TEXT_MUTED: u32 = 0xFF64748B;
const CARD_RADIUS: usize = 20;

#[inline]
fn card_pixel(rect: &Rect, x: usize, y: usize) -> Option<u32> {
    if !round_contains(rect, CARD_RADIUS, x, y) {
        return None;
    }
    let inner = rect.inset(1);
    if !round_contains(&inner, CARD_RADIUS.saturating_sub(1), x, y) {
        return Some(CARD_BORDER);
    }
    Some(CARD_BG)
}

// -----------------------------------------------------------------------------
// Screen 6 Presentation Entrypoint
// -----------------------------------------------------------------------------

pub fn get_system_space_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Status Bar
    if y < STATUS_BAR_HEIGHT {
        if state.clock_source_ok {
            if let Some(c) = test_text_aa(x, y, 48, 14, 1, &state.clock_time, TEXT_DARK, 0xFFFFFFFF) {
                return c;
            }
        }
        // Battery status icon on top right
        if x >= 640 && x <= 672 && y >= 16 && y <= 28 {
            if let Some(c) = draw_battery_icon(x, y, 656, 22, TEXT_DARK) {
                return c;
            }
        }
        return BG_COLOR;
    }
    if y == STATUS_BAR_HEIGHT {
        return CARD_BORDER;
    }

    // 2. Back control, in the reference's position and size.
    //
    // WHY it was missing: the Settings app opens this same surface with a `<`
    // pill at the top-left, and the reference screen has one too, so a user who
    // reached System Space from the home surface's search bar had no control to
    // come back with -- the only way out was a gesture. A control that is
    // airtight on one route in and absent on another is a gap, not a style.
    //
    // WHY this shape: the same 48x48 soft pill with a 12-radius corner and a
    // 2px chevron the Settings app draws, so the two agree where a user would
    // compare them. `back_control_pixel` is local because the shell and the
    // Settings app cannot share a module, and a copy that matches is better than
    // a dependency that would couple the shell's layout to an app's.
    if let Some(c) = back_control_pixel(x, y) {
        return c;
    }

    // 3. Large Bold Title: "System Space"
    // WHY y0 = 52 and a 72-row gate: a scale-3 run needs GLYPH_H * scale rows, and
    // the old `y >= 56 && y < 96` window threw away the bottom 36 of 72, so the
    // title rendered as its own severed upper halves. The run now sits high enough
    // that its ink (61..109) clears both the status rule at y=48 and the sphere
    // band that starts at y=115. The x window was 240 while the word measures
    // ~261px at this scale, so the final `m` was being cut as well.
    //
    // WHY the word is "System Space" and not "System": the Settings app titles
    // this same surface "System Space", so a title of "System" read as a
    // different screen to a user who had arrived from Settings -- and the name is
    // the design system's own.
    //
    // WHY the window is 576 wide: measured with the face's own advance table,
    // `text::measure(b"System Space", 3)` is 480px, and the run starts at x 48,
    // so it ends at 528. 576 leaves 48px of slack on the right at the surface's
    // own margin, and admits the whole word. The old 320-wide window was cut for
    // "System" and would have clipped this at "Spac".
    //
    // WHY the title still clears the orb band: its ink ends at row 109 and the
    // band the orb occupies starts at 115, so widening the word horizontally
    // costs nothing vertically.
    if y >= ORB_BAND_Y0 - 88 && y < ORB_BAND_Y1 - 126 && x >= 48 && x < 576 {
        if let Some(c) = test_text_aa(x, y, 48, 112, 3, b"System Space", TEXT_DARK, 0xFFFFFFFF) {
            return c;
        }
    }

    // 3. Central 3D Shaded Sphere (cx: 360, cy: 175, r: 52)
    if y >= ORB_BAND_Y0 && y <= ORB_BAND_Y1 && x >= 290 && x <= 430 {
        if let Some(c) = draw_orb(x, y, 360, 255, 52, unsafe { crate::ORB_TICKS }) {
            return c;
        }
    }

    // Subtitle under sphere: "Everything in one place"
    if y >= 324 && y < 348 && x >= 240 && x < 480 {
        if let Some(c) = test_text_aa(x, y, 252, 324, 1, b"Everything in one place", TEXT_MUTED, 0xFFFFFFFF) {
            return c;
        }
    }

    // 4. Row 1 Cards
    // Left: Device
    if let Some(bg) = card_pixel(&CARD_R1_L, x, y) {
        if let Some(ic) = draw_device_icon(x, y, 75, CARD_R1_L.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 105, CARD_R1_L.y + CARD_TITLE_DY, 2, b"Device", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        if let Some(c) = test_text_aa(
            x, y, 65, CARD_R1_L.y + CARD_CAPTION_DY, 1, b"Display, Sound, Gestures", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }
    // Right: Network
    if let Some(bg) = card_pixel(&CARD_R1_R, x, y) {
        if let Some(ic) = draw_network_icon(x, y, 410, CARD_R1_R.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 440, CARD_R1_R.y + CARD_TITLE_DY, 2, b"Network", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        if let Some(c) = test_text_aa(
            x, y, 400, CARD_R1_R.y + CARD_CAPTION_DY, 1, b"Wi-Fi, Mobile, Bluetooth", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }

    // 5. Row 2 Cards
    // Left: Personal
    if let Some(bg) = card_pixel(&CARD_R2_L, x, y) {
        if let Some(ic) = draw_personal_icon(x, y, 75, CARD_R2_L.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 105, CARD_R2_L.y + CARD_TITLE_DY, 2, b"Personal", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        if let Some(c) = test_text_aa(
            x, y, 65, CARD_R2_L.y + CARD_CAPTION_DY, 1, b"Accounts, Language, Time", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }
    // Right: Apps
    if let Some(bg) = card_pixel(&CARD_R2_R, x, y) {
        if let Some(ic) = draw_apps_icon(x, y, 410, CARD_R2_R.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 440, CARD_R2_R.y + CARD_TITLE_DY, 2, b"Apps", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        if let Some(c) = test_text_aa(
            x, y, 400, CARD_R2_R.y + CARD_CAPTION_DY, 1, b"Permissions, Storage", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }

    // 6. Row 3 Cards
    // Left: Power (POWER_ENTRY)
    if let Some(bg) = card_pixel(&CARD_R3_L, x, y) {
        if let Some(ic) = draw_battery_icon(x, y, 75, CARD_R3_L.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 105, CARD_R3_L.y + CARD_TITLE_DY, 2, b"Power", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        // WHY "Power options" and not "Charging": the subtitle names the section,
        // and "Charging" would claim a live charge state this shell never measured.
        if let Some(c) = test_text_aa(
            x, y, 65, CARD_R3_L.y + CARD_CAPTION_DY, 1, b"Battery, Power options", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }
    // Right: Security
    if let Some(bg) = card_pixel(&CARD_R3_R, x, y) {
        if let Some(ic) = draw_security_icon(x, y, 410, CARD_R3_R.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 440, CARD_R3_R.y + CARD_TITLE_DY, 2, b"Security", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        if let Some(c) = test_text_aa(
            x, y, 400, CARD_R3_R.y + CARD_CAPTION_DY, 1, b"Lock, Encryption", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }

    // 7. Row 4 Card: System (Full width)
    if let Some(bg) = card_pixel(&CARD_R4, x, y) {
        if let Some(ic) = draw_gear_icon(x, y, 75, CARD_R4.y + CARD_ICON_DY, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text_aa(
            x, y, 105, CARD_R4.y + CARD_TITLE_DY, 2, b"System", TEXT_DARK, 0xFFFFFFFF,
        ) {
            return c;
        }
        if let Some(c) = test_text_aa(
            x, y, 105, CARD_R4.y + CARD_CAPTION_DY, 1, b"Updates, About, Tools", TEXT_MUTED,
            0xFFFFFFFF,
        ) {
            return c;
        }
        return bg;
    }

    // 8. Home indicator bar at bottom
    if y >= 1416 && y <= 1422 && x >= 260 && x <= 460 {
        return 0xFF94A3B8;
    }

    BG_COLOR
}
