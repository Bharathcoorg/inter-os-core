//! Power Menu: full-screen modern modal sheet with Power off, Restart, Lock screen, and Emergency.
//!
//! Production flagship quality with large circular action buttons, bold typography,
//! color accents, and full integration with system power transitions and lock screen.

#![allow(unused_imports, dead_code)]

use super::draw::round_contains;
use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::{type_scale, Theme};
use crate::screens::home;

const COLOR_BG_TOP: u32 = launcher::theme::SHELL_DARK.surface;
const COLOR_BG_BOT: u32 = 0xFF05070A;

const SHEET_RECT: Rect = Rect::new(40, 60, 640, 545);
const SHEET_RADIUS: usize = 32;
const SHEET_FILL: u32 = 0xFF12151C;
const SHEET_BORDER: u32 = 0xFF282F3D;

const HANDLE_RECT: Rect = Rect::new(330, 76, 60, 5);
const HANDLE_COLOR: u32 = 0xFF3B4454;

// Circle button geometry: radius 36 (72x72px diameter)
const BTN_RADIUS: i32 = 36;
const BTN_R2: i32 = BTN_RADIUS * BTN_RADIUS;

// Centers for the 2x2 grid
const COL0_CX: usize = 220;
const COL1_CX: usize = 500;
const ROW0_CY: usize = 240;
const ROW1_CY: usize = 380;

// Button 1: Power Off (Col 0, Row 0)
const BTN_POWER_CX: usize = COL0_CX;
const BTN_POWER_CY: usize = ROW0_CY;
const COLOR_POWER_FILL: u32 = launcher::theme::SHELL_DARK.emergency;
const COLOR_POWER_RING: u32 = 0xFFEF4444;

// Button 2: Lock Screen (Col 1, Row 0) - (500, 240) contains (500, 225)
const BTN_LOCK_CX: usize = COL1_CX;
const BTN_LOCK_CY: usize = ROW0_CY;
const COLOR_LOCK_FILL: u32 = 0xFF334155;
const COLOR_LOCK_RING: u32 = 0xFF475569;

// Button 3: Restart (Col 0, Row 1)
const BTN_RESTART_CX: usize = COL0_CX;
const BTN_RESTART_CY: usize = ROW1_CY;
const COLOR_RESTART_FILL: u32 = 0xFF0369A1;
const COLOR_RESTART_RING: u32 = 0xFF0EA5E9;

// Button 4: Emergency (Col 1, Row 1)
const BTN_EMERGENCY_CX: usize = COL1_CX;
const BTN_EMERGENCY_CY: usize = ROW1_CY;
const COLOR_EMERGENCY_FILL: u32 = 0xFF991B1B;
const COLOR_EMERGENCY_RING: u32 = 0xFFB91C1C;

// Cancel button
pub const CANCEL_RECT: Rect = Rect::new(200, 524, 320, 54);
const CANCEL_RADIUS: usize = 27;
const COLOR_CANCEL_FILL: u32 = 0xFF1E2430;
const COLOR_CANCEL_BORDER: u32 = 0xFF333E50;
const COLOR_CANCEL_TEXT: u32 = 0xFFE2E8F0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMenuAction {
    PowerOff,
    LockScreen,
    Restart,
    Emergency,
    Cancel,
    Dismiss,
}

pub fn power_menu_hit(x: usize, y: usize) -> PowerMenuAction {
    // 1. Power off: Col 0, Row 0
    if (80..340).contains(&x) && (190..340).contains(&y) {
        return PowerMenuAction::PowerOff;
    }
    // 2. Lock screen: Col 1, Row 0 (contains (500, 225)!)
    if (360..640).contains(&x) && (190..340).contains(&y) {
        return PowerMenuAction::LockScreen;
    }
    // 3. Restart: Col 0, Row 1
    if (80..340).contains(&x) && (340..490).contains(&y) {
        return PowerMenuAction::Restart;
    }
    // 4. Emergency: Col 1, Row 1
    if (360..640).contains(&x) && (340..490).contains(&y) {
        return PowerMenuAction::Emergency;
    }
    // 5. Cancel pill button
    if (180..540).contains(&x) && (510..590).contains(&y) {
        return PowerMenuAction::Cancel;
    }
    // Outside sheet / backdrop
    PowerMenuAction::Dismiss
}

/// The colour of (`x`, `y`) when the run at (`x0`, `y0`) on `scale` inks it,
/// drawn by the outline rasteriser when it can place the run and by the bitmap
/// face when it cannot.
fn text_pixel(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    bytes: &[u8],
    ink: u32,
    bg: u32,
) -> Option<u32> {
    let em = launcher::outline_text::em_for_scale(scale);
    let (asc, desc) = interos_std::design::typeface::line_metrics(em);
    let baseline = (y0 + launcher::font::ascent_for_scale(scale)).max(asc as usize);
    let top = baseline.saturating_sub(asc as usize);
    if x < x0 || y < top || y >= baseline + desc as usize {
        return None;
    }
    if scale >= 1 && bytes.len() <= launcher::outline_text::KEY_MAX {
        if let Some(run) = launcher::outline_text::OutlineRun::place(x0, baseline, em, bytes) {
            return run.pixel(x, y, ink, bg);
        }
    }
    test_text_aa(x, y, x0, y0, scale, bytes, ink, bg)
}

#[inline]
fn draw_lock_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);

    // Shackle at top: dy in -16..-3, dx in -8..8
    if dy >= -16 && dy <= -3 && dx >= -8 && dx <= 8 {
        let is_shackle = if dy < -9 {
            let ady = dy + 9;
            let d2 = dx * dx + ady * ady;
            d2 >= 36 && d2 <= 64
        } else {
            dx.abs() >= 6 && dx.abs() <= 8
        };
        if is_shackle {
            return Some(tint);
        }
    }

    // Body at bottom: dy in -3..14, dx in -12..12
    if dy >= -3 && dy <= 14 && dx >= -12 && dx <= 12 {
        let is_border = dx.abs() >= 10 || dy <= -1 || dy >= 12;
        let is_keyhole = (dx.abs() <= 2 && dy >= 3 && dy <= 8) || (dx.abs() <= 1 && dy >= 8 && dy <= 10);
        if is_border || is_keyhole {
            return Some(tint);
        }
    }

    None
}

#[inline]
fn draw_restart_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;

    // Circular ring arc, gap at top-right
    if d2 >= 121 && d2 <= 225 {
        if !(dx > 2 && dy < -4) {
            return Some(tint);
        }
    }

    // Arrowhead at top-right pointing clockwise
    if dx >= 3 && dx <= 13 && dy >= -16 && dy <= -6 {
        if (dx - dy >= 15 && dx <= 12) || (dx >= 7 && dy >= -10) {
            return Some(tint);
        }
    }

    None
}

#[inline]
fn draw_power_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;

    // Circle ring with top gap
    if d2 >= 121 && d2 <= 225 {
        if !(dx.abs() <= 4 && dy < 0) {
            return Some(tint);
        }
    }

    // Vertical line in top gap
    if dx.abs() <= 2 && dy >= -16 && dy <= -2 {
        return Some(tint);
    }

    None
}

#[inline]
fn draw_emergency_icon(x: usize, y: usize, cx: usize, cy: usize, tint: u32) -> Option<u32> {
    let dx = (x as i32) - (cx as i32);
    let dy = (y as i32) - (cy as i32);
    let d2 = dx * dx + dy * dy;

    // Central hub
    if d2 <= 25 {
        return Some(tint);
    }

    // 8-spoke medical / alert star
    let is_v = dx.abs() <= 3 && dy.abs() <= 15;
    let is_h = dy.abs() <= 3 && dx.abs() <= 15;
    let is_d1 = (dx - dy).abs() <= 2 && dx.abs() <= 11 && dy.abs() <= 11;
    let is_d2 = (dx + dy).abs() <= 2 && dx.abs() <= 11 && dy.abs() <= 11;

    if is_v || is_h || is_d1 || is_d2 {
        return Some(tint);
    }

    None
}

pub fn get_power_menu_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Top status bar (y < 48)
    //
    // WHY this screen reuses the home strip instead of painting its own: the
    // sheet covers the surface with an opaque backdrop, so it has to draw a
    // status bar, and a second copy is a second set of geometry and a second
    // set of readings. `home` owns the one clock position and the one unbound
    // telemetry glyph set.
    if y < 48 {
        if state.clock_source_ok {
            if let Some(c) = text_pixel(
                x,
                y,
                40,
                8,
                type_scale::BODY,
                &state.clock_time,
                0xFFCBD5E1,
                COLOR_BG_TOP,
            ) {
                return c;
            }
        }
        if let Some(c) = crate::screens::status::marks(x, y, 0xFFCBD5E1) {
            return c;
        }
        return COLOR_BG_TOP;
    }

    // 2. Home bottom gesture bar (y: 1410..1416)
    if y >= 1410 && y <= 1416 && x >= 260 && x <= 460 {
        return 0x55FFFFFF;
    }

    // 3. Main Modal Sheet Drawer
    if round_contains(&SHEET_RECT, SHEET_RADIUS, x, y) {
        let inner = SHEET_RECT.inset(1);
        if !round_contains(&inner, SHEET_RADIUS.saturating_sub(1), x, y) {
            return SHEET_BORDER;
        }

        // Top drag handle pill
        if round_contains(&HANDLE_RECT, 2, x, y) {
            return HANDLE_COLOR;
        }

        // Header Title: "Power & Safety" (scale 4 bold white)
        let title_bytes = b"Power & Safety";
        let title_w = text::measure(title_bytes, 4);
        let title_x = (720 - title_w) / 2;
        if let Some(c) = text_pixel(x, y, title_x, 96, 4, title_bytes, 0xFFFFFFFF, SHEET_FILL) {
            return c;
        }

        // Header Description: "Select an option or tap Cancel to return"
        let desc_bytes = b"Select an option or tap Cancel to return";
        let desc_w = text::measure(desc_bytes, 2);
        let desc_x = (720 - desc_w) / 2;
        if let Some(c) = text_pixel(x, y, desc_x, 158, 2, desc_bytes, 0xFF94A3B8, SHEET_FILL) {
            return c;
        }

        // Button 1: Power Off (cx=220, cy=240, R=36)
        let p_dx = (x as i32) - (BTN_POWER_CX as i32);
        let p_dy = (y as i32) - (BTN_POWER_CY as i32);
        let p_d2 = p_dx * p_dx + p_dy * p_dy;
        if p_d2 <= BTN_R2 {
            if let Some(c) = draw_power_icon(x, y, BTN_POWER_CX, BTN_POWER_CY, 0xFFFFFFFF) {
                return c;
            }
            if p_d2 >= (BTN_RADIUS - 2) * (BTN_RADIUS - 2) {
                return COLOR_POWER_RING;
            }
            return COLOR_POWER_FILL;
        }

        // Button 1 Labels: "Power off" (scale 2) & "Shut down" (scale 1)
        let p_label_bytes = b"Power off";
        let p_label_w = text::measure(p_label_bytes, 3);
        let p_label_x = BTN_POWER_CX.saturating_sub(p_label_w / 2);
        if let Some(c) = text_pixel(x, y, p_label_x, 276, 3, p_label_bytes, 0xFFFFFFFF, SHEET_FILL) {
            return c;
        }
        let p_sub_bytes = b"Shut down";
        let p_sub_w = text::measure(p_sub_bytes, 2);
        let p_sub_x = BTN_POWER_CX.saturating_sub(p_sub_w / 2);
        if let Some(c) = text_pixel(x, y, p_sub_x, 312, 2, p_sub_bytes, 0xFFCBD5E1, SHEET_FILL) {
            return c;
        }

        // Button 2: Lock Screen (cx=500, cy=240, R=36)
        let l_dx = (x as i32) - (BTN_LOCK_CX as i32);
        let l_dy = (y as i32) - (BTN_LOCK_CY as i32);
        let l_d2 = l_dx * l_dx + l_dy * l_dy;
        if l_d2 <= BTN_R2 {
            if let Some(c) = draw_lock_icon(x, y, BTN_LOCK_CX, BTN_LOCK_CY, 0xFFFFFFFF) {
                return c;
            }
            if l_d2 >= (BTN_RADIUS - 2) * (BTN_RADIUS - 2) {
                return COLOR_LOCK_RING;
            }
            return COLOR_LOCK_FILL;
        }

        // Button 2 Labels: "Lock screen" (scale 2) & "Secure device" (scale 1)
        let l_label_bytes = b"Lock screen";
        let l_label_w = text::measure(l_label_bytes, 3);
        let l_label_x = BTN_LOCK_CX.saturating_sub(l_label_w / 2);
        if let Some(c) = text_pixel(x, y, l_label_x, 276, 3, l_label_bytes, 0xFFFFFFFF, SHEET_FILL) {
            return c;
        }
        let l_sub_bytes = b"Secure device";
        let l_sub_w = text::measure(l_sub_bytes, 2);
        let l_sub_x = BTN_LOCK_CX.saturating_sub(l_sub_w / 2);
        if let Some(c) = text_pixel(x, y, l_sub_x, 312, 2, l_sub_bytes, 0xFFCBD5E1, SHEET_FILL) {
            return c;
        }

        // Button 3: Restart (cx=220, cy=380, R=36)
        let r_dx = (x as i32) - (BTN_RESTART_CX as i32);
        let r_dy = (y as i32) - (BTN_RESTART_CY as i32);
        let r_d2 = r_dx * r_dx + r_dy * r_dy;
        if r_d2 <= BTN_R2 {
            if let Some(c) = draw_restart_icon(x, y, BTN_RESTART_CX, BTN_RESTART_CY, 0xFFFFFFFF) {
                return c;
            }
            if r_d2 >= (BTN_RADIUS - 2) * (BTN_RADIUS - 2) {
                return COLOR_RESTART_RING;
            }
            return COLOR_RESTART_FILL;
        }

        // Button 3 Labels: "Restart" (scale 2) & "Reboot OS" (scale 1)
        let r_label_bytes = b"Restart";
        let r_label_w = text::measure(r_label_bytes, 3);
        let r_label_x = BTN_RESTART_CX.saturating_sub(r_label_w / 2);
        if let Some(c) = text_pixel(x, y, r_label_x, 416, 3, r_label_bytes, 0xFFFFFFFF, SHEET_FILL) {
            return c;
        }
        let r_sub_bytes = b"Reboot OS";
        let r_sub_w = text::measure(r_sub_bytes, 2);
        let r_sub_x = BTN_RESTART_CX.saturating_sub(r_sub_w / 2);
        if let Some(c) = text_pixel(x, y, r_sub_x, 452, 2, r_sub_bytes, 0xFFCBD5E1, SHEET_FILL) {
            return c;
        }

        // Button 4: Emergency (cx=500, cy=380, R=36)
        let e_dx = (x as i32) - (BTN_EMERGENCY_CX as i32);
        let e_dy = (y as i32) - (BTN_EMERGENCY_CY as i32);
        let e_d2 = e_dx * e_dx + e_dy * e_dy;
        if e_d2 <= BTN_R2 {
            if let Some(c) = draw_emergency_icon(x, y, BTN_EMERGENCY_CX, BTN_EMERGENCY_CY, 0xFFFFFFFF) {
                return c;
            }
            if e_d2 >= (BTN_RADIUS - 2) * (BTN_RADIUS - 2) {
                return COLOR_EMERGENCY_RING;
            }
            return COLOR_EMERGENCY_FILL;
        }

        // Button 4 Labels: "Emergency" (scale 2) & "SOS / Dialer" (scale 1)
        let e_label_bytes = b"Emergency";
        let e_label_w = text::measure(e_label_bytes, 3);
        let e_label_x = BTN_EMERGENCY_CX.saturating_sub(e_label_w / 2);
        if let Some(c) = text_pixel(x, y, e_label_x, 416, 3, e_label_bytes, 0xFFFFFFFF, SHEET_FILL) {
            return c;
        }
        let e_sub_bytes = b"SOS / Dialer";
        let e_sub_w = text::measure(e_sub_bytes, 2);
        let e_sub_x = BTN_EMERGENCY_CX.saturating_sub(e_sub_w / 2);
        if let Some(c) = text_pixel(x, y, e_sub_x, 452, 2, e_sub_bytes, 0xFFFCA5A5, SHEET_FILL) {
            return c;
        }

        // Cancel Pill Button (y: 524..578, x: 200..520)
        if round_contains(&CANCEL_RECT, CANCEL_RADIUS, x, y) {
            let c_inner = CANCEL_RECT.inset(1);
            if !round_contains(&c_inner, CANCEL_RADIUS.saturating_sub(1), x, y) {
                return COLOR_CANCEL_BORDER;
            }
            let cancel_bytes = b"Cancel";
            let c_w = text::measure(cancel_bytes, 3);
            let c_x = CANCEL_RECT.x + (CANCEL_RECT.w - c_w) / 2;
            if let Some(c) = text_pixel(x, y, c_x, 533, 3, cancel_bytes, COLOR_CANCEL_TEXT, COLOR_CANCEL_FILL) {
                return c;
            }
            return COLOR_CANCEL_FILL;
        }

        return SHEET_FILL;
    }

    // Smooth subtle vertical backdrop gradient outside the sheet
    let t = (y.saturating_sub(48) * 255) / (1440 - 48);
    let r1 = (COLOR_BG_TOP >> 16) & 0xFF;
    let g1 = (COLOR_BG_TOP >> 8) & 0xFF;
    let b1 = COLOR_BG_TOP & 0xFF;
    let r2 = (COLOR_BG_BOT >> 16) & 0xFF;
    let g2 = (COLOR_BG_BOT >> 8) & 0xFF;
    let b2 = COLOR_BG_BOT & 0xFF;
    // WHY i64 and a clamp: the delta is signed, and the old i32-to-u32 cast of a
    // negative quotient panicked on add in debug builds when the bottom stop was
    // darker than the top. Interpolating in i64 and clamping keeps every channel
    // inside 0..=255 for either stop order.
    let lerp = |a: u32, b: u32| -> u32 {
        let v = a as i64 + ((b as i64 - a as i64) * t as i64 / 255);
        v.clamp(0, 255) as u32
    };
    let r = lerp(r1, r2);
    let g = lerp(g1, g2);
    let b = lerp(b1, b2);

    0xFF000000 | (r << 16) | (g << 8) | b
}
