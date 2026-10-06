//! System Space: device, network, personal, apps, power, and security settings.
//!
//! Pixel-perfect reference implementation for Screen 6 matching the Sovereign
//! InterOS Core reference architecture (media_1791278018153.jpg).

#![allow(unused_imports, dead_code)]

use crate::screens::lock::LOCK_DATE_TEXT_LEN;
use crate::{test_text, LauncherState};
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
    let w = text::measure(run, scale);
    if y < y0 || y >= y0 + h || x < rect.x || x >= rect.x + w {
        return None;
    }
    test_text(x, y, rect.x, y0, scale, run, color)
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
    if !cell.contains(x, y) {
        return theme.bg;
    }
    let inner = cell.rect.inset(1);
    if !round_contains(&inner, cell.corner.saturating_sub(1), x, y) {
        return theme.border;
    }

    let pad = cell.rect.inset(space::MD);
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
        if dx <= 2 {
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

#[inline]
fn draw_sphere(x: usize, y: usize, cx: usize, cy: usize, r: usize) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;
    let r2 = (r * r) as i32;

    // Contact drop shadow below sphere
    let shadow_dy = dy - 56;
    let s_dx = dx;
    let s_dist = s_dx * s_dx * 100 / (60 * 60) + shadow_dy * shadow_dy * 100 / (10 * 10);
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

        let dot = (-3 * dx - 4 * dy + 4 * z) / (r as i32);
        let diffuse = dot.max(0).min(6);

        let spec_dx = dx + 15;
        let spec_dy = dy + 16;
        let spec_d2 = spec_dx * spec_dx + spec_dy * spec_dy;

        let mut cr = 24 + diffuse * 8;
        let mut cg = 30 + diffuse * 10;
        let mut cb = 42 + diffuse * 14;

        if z <= 12 {
            let rim = (12 - z) * 3;
            cr += rim;
            cg += rim + 2;
            cb += rim + 5;
        }

        if spec_d2 <= 144 {
            let spec_intensity = (144 - spec_d2) as i32;
            cr += spec_intensity * 120 / 144;
            cg += spec_intensity * 135 / 144;
            cb += spec_intensity * 155 / 144;
        }
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

const CARD_R1_L: Rect = Rect::new(40, 295, 305, 120);
const CARD_R1_R: Rect = Rect::new(375, 295, 305, 120);
const CARD_R2_L: Rect = Rect::new(40, 435, 305, 120);
const CARD_R2_R: Rect = Rect::new(375, 435, 305, 120);
const CARD_R3_L: Rect = Rect::new(40, 590, 305, 120); // POWER_ENTRY
const CARD_R3_R: Rect = Rect::new(375, 590, 305, 120);
const CARD_R4: Rect = Rect::new(40, 730, 640, 110);

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
            if let Some(c) = test_text(x, y, 48, 14, 1, &state.clock_time, TEXT_DARK) {
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

    // 2. Large Bold Title: "System"
    if y >= 56 && y < 96 && x >= 48 && x < 240 {
        if let Some(c) = test_text(x, y, 48, 60, 3, b"System", TEXT_DARK) {
            return c;
        }
    }

    // 3. Central 3D Shaded Sphere (cx: 360, cy: 175, r: 52)
    if y >= 115 && y <= 245 && x >= 290 && x <= 430 {
        if let Some(c) = draw_sphere(x, y, 360, 175, 52) {
            return c;
        }
    }

    // Subtitle under sphere: "Everything in one place"
    if y >= 248 && y < 270 && x >= 240 && x < 480 {
        if let Some(c) = test_text(x, y, 252, 250, 1, b"Everything in one place", TEXT_MUTED) {
            return c;
        }
    }

    // 4. Row 1 Cards (y: 295..415)
    // Left: Device
    if let Some(bg) = card_pixel(&CARD_R1_L, x, y) {
        if let Some(ic) = draw_device_icon(x, y, 75, 332, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 105, 322, 2, b"Device", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 65, 372, 1, b"Display, Sound, Gestures", TEXT_MUTED) {
            return c;
        }
        return bg;
    }
    // Right: Network
    if let Some(bg) = card_pixel(&CARD_R1_R, x, y) {
        if let Some(ic) = draw_network_icon(x, y, 410, 332, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 440, 322, 2, b"Network", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 400, 372, 1, b"Wi-Fi, Mobile, Bluetooth", TEXT_MUTED) {
            return c;
        }
        return bg;
    }

    // 5. Row 2 Cards (y: 435..555)
    // Left: Personal
    if let Some(bg) = card_pixel(&CARD_R2_L, x, y) {
        if let Some(ic) = draw_personal_icon(x, y, 75, 472, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 105, 462, 2, b"Personal", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 65, 512, 1, b"Accounts, Language, Time", TEXT_MUTED) {
            return c;
        }
        return bg;
    }
    // Right: Apps
    if let Some(bg) = card_pixel(&CARD_R2_R, x, y) {
        if let Some(ic) = draw_apps_icon(x, y, 410, 472, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 440, 462, 2, b"Apps", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 400, 512, 1, b"Permissions, Storage", TEXT_MUTED) {
            return c;
        }
        return bg;
    }

    // 6. Row 3 Cards (y: 590..710)
    // Left: Power (POWER_ENTRY)
    if let Some(bg) = card_pixel(&CARD_R3_L, x, y) {
        if let Some(ic) = draw_battery_icon(x, y, 75, 627, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 105, 617, 2, b"Power", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 65, 667, 1, b"Battery, Charging", TEXT_MUTED) {
            return c;
        }
        return bg;
    }
    // Right: Security
    if let Some(bg) = card_pixel(&CARD_R3_R, x, y) {
        if let Some(ic) = draw_security_icon(x, y, 410, 627, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 440, 617, 2, b"Security", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 400, 667, 1, b"Lock, Encryption", TEXT_MUTED) {
            return c;
        }
        return bg;
    }

    // 7. Row 4 Card: System (Full width 40..680, y: 730..840)
    if let Some(bg) = card_pixel(&CARD_R4, x, y) {
        if let Some(ic) = draw_gear_icon(x, y, 75, 785, TEXT_DARK) {
            return ic;
        }
        if let Some(c) = test_text(x, y, 105, 765, 2, b"System", TEXT_DARK) {
            return c;
        }
        if let Some(c) = test_text(x, y, 105, 800, 1, b"Updates, About, Tools", TEXT_MUTED) {
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