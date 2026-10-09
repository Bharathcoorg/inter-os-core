//! Power Menu: full-screen modern modal with Lock, Restart, Power off, and Emergency.
//!
//! Designed to match the high-definition reference design (`target/ref_screen_8.png`).

#![allow(unused_imports, dead_code)]

use super::system_space::round_contains;
use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::text;

const COLOR_BG_TOP: u32 = 0xFF111214;
const COLOR_BG_BOT: u32 = 0xFF111214;

const COLOR_CARD_FILL: u32 = 0xFF25282A;
const COLOR_CARD_BORDER: u32 = 0xFF3A3D40;
const COLOR_CARD_TEXT: u32 = 0xFFF7F6F2;

const COLOR_EMERGENCY_FILL: u32 = 0xFF2B1416;
const COLOR_EMERGENCY_BORDER: u32 = 0xFF582328;
const COLOR_EMERGENCY_TEXT: u32 = 0xFFFF5555;

const COLOR_CANCEL_FILL: u32 = 0xFF25282A;
const COLOR_CANCEL_BORDER: u32 = 0xFF3A3D40;
const COLOR_CANCEL_TEXT: u32 = 0xFF777773;

const CARD_X: usize = 60;
const CARD_W: usize = 600;
const CARD_H: usize = 75;
const CARD_RADIUS: usize = 24;

const CARD1_RECT: Rect = Rect::new(CARD_X, 190, CARD_W, CARD_H);
const CARD2_RECT: Rect = Rect::new(CARD_X, 285, CARD_W, CARD_H);
const CARD3_RECT: Rect = Rect::new(CARD_X, 380, CARD_W, CARD_H);
const CARD4_RECT: Rect = Rect::new(CARD_X, 475, CARD_W, CARD_H);
const CANCEL_RECT: Rect = Rect::new(240, 580, 240, 55);

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

    // Body at bottom: dy in -3..13, dx in -12..12
    if dy >= -3 && dy <= 13 && dx >= -12 && dx <= 12 {
        let is_border = dx <= -10 || dx >= 10 || dy <= -1 || dy >= 11;
        let is_keyhole = dx.abs() <= 2 && dy >= 2 && dy <= 7;
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
    if d2 >= 100 && d2 <= 169 {
        if !(dx > 2 && dy < -5) {
            return Some(tint);
        }
    }

    // Arrowhead at top-right pointing clockwise
    if dx >= 4 && dx <= 12 && dy >= -15 && dy <= -6 {
        if (dx - dy >= 16 && dx <= 11) || (dx >= 8 && dy >= -9) {
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
    if d2 >= 100 && d2 <= 169 {
        if !(dx.abs() <= 5 && dy < 0) {
            return Some(tint);
        }
    }

    // Vertical line in top gap
    if dx.abs() <= 2 && dy >= -15 && dy <= -2 {
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
    if d2 <= 16 {
        return Some(tint);
    }

    // 8-spoke medical / alert star
    let is_v = dx.abs() <= 2 && dy.abs() <= 13;
    let is_h = dy.abs() <= 2 && dx.abs() <= 13;
    let is_d1 = (dx - dy).abs() <= 2 && dx.abs() <= 10 && dy.abs() <= 10;
    let is_d2 = (dx + dy).abs() <= 2 && dx.abs() <= 10 && dy.abs() <= 10;

    if is_v || is_h || is_d1 || is_d2 {
        return Some(tint);
    }

    None
}

pub fn get_power_menu_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // 1. Top status bar (y < 48)
    if y < 48 {
        if state.clock_source_ok {
            if let Some(c) = test_text_aa(x, y, 48, 14, 1, &state.clock_time, 0xFFCBD5E1, 0xFF111214) {
                return c;
            }
        }
        // Battery glyph outline + terminal at (650, 16). WHY outline only: there
        // is no fuel gauge bound to this shell, so a filled cell would be a
        // measurement the shell never took.
        if x >= 648 && x <= 672 && y >= 17 && y <= 31 {
            let is_body = x <= 668 && (x == 648 || x == 668 || y == 17 || y == 31);
            let is_terminal = x >= 669 && y >= 21 && y <= 27;
            if is_body || is_terminal {
                return 0xFFCBD5E1;
            }
        }
        return COLOR_BG_TOP;
    }

    // 2. Card 1: Lock device (y: 190..265)
    if round_contains(&CARD1_RECT, CARD_RADIUS, x, y) {
        let inner = CARD1_RECT.inset(1);
        if !round_contains(&inner, CARD_RADIUS.saturating_sub(1), x, y) {
            return COLOR_CARD_BORDER;
        }
        // Lock vector icon at cx=115, cy=227
        if let Some(c) = draw_lock_icon(x, y, 115, 227, COLOR_CARD_TEXT) {
            return c;
        }
        // Single centered title text "Lock device" (scale 1, 24px height centered vertically at 190 + 25 = 215)
        if let Some(c) = test_text_aa(x, y, 175, 215, 1, b"Lock device", COLOR_CARD_TEXT, 0xFF111214) {
            return c;
        }
        return COLOR_CARD_FILL;
    }

    // 3. Card 2: Restart (y: 285..360)
    if round_contains(&CARD2_RECT, CARD_RADIUS, x, y) {
        let inner = CARD2_RECT.inset(1);
        if !round_contains(&inner, CARD_RADIUS.saturating_sub(1), x, y) {
            return COLOR_CARD_BORDER;
        }
        // Restart vector icon at cx=115, cy=322
        if let Some(c) = draw_restart_icon(x, y, 115, 322, COLOR_CARD_TEXT) {
            return c;
        }
        // Single centered title text "Restart"
        if let Some(c) = test_text_aa(x, y, 175, 310, 1, b"Restart", COLOR_CARD_TEXT, 0xFF111214) {
            return c;
        }
        return COLOR_CARD_FILL;
    }

    // 4. Card 3: Power off (y: 380..455)
    if round_contains(&CARD3_RECT, CARD_RADIUS, x, y) {
        let inner = CARD3_RECT.inset(1);
        if !round_contains(&inner, CARD_RADIUS.saturating_sub(1), x, y) {
            return COLOR_CARD_BORDER;
        }
        // Power vector icon at cx=115, cy=417
        if let Some(c) = draw_power_icon(x, y, 115, 417, COLOR_CARD_TEXT) {
            return c;
        }
        // Single centered title text "Power off"
        if let Some(c) = test_text_aa(x, y, 175, 405, 1, b"Power off", COLOR_CARD_TEXT, 0xFF111214) {
            return c;
        }
        return COLOR_CARD_FILL;
    }

    // 5. Card 4: Emergency (y: 475..550)
    if round_contains(&CARD4_RECT, CARD_RADIUS, x, y) {
        let inner = CARD4_RECT.inset(1);
        if !round_contains(&inner, CARD_RADIUS.saturating_sub(1), x, y) {
            return COLOR_EMERGENCY_BORDER;
        }
        // Emergency vector icon at cx=115, cy=512
        if let Some(c) = draw_emergency_icon(x, y, 115, 512, COLOR_EMERGENCY_TEXT) {
            return c;
        }
        // Single centered title text "Emergency"
        if let Some(c) = test_text_aa(x, y, 175, 500, 1, b"Emergency", COLOR_EMERGENCY_TEXT, 0xFF111214) {
            return c;
        }
        return COLOR_EMERGENCY_FILL;
    }

    // 6. Cancel pill button (y: 580..635, x: 240..480)
    if round_contains(&CANCEL_RECT, 27, x, y) {
        let inner = CANCEL_RECT.inset(1);
        if !round_contains(&inner, 26, x, y) {
            return COLOR_CANCEL_BORDER;
        }
        if let Some(c) = test_text_aa(x, y, 325, 595, 1, b"Cancel", COLOR_CANCEL_TEXT, 0xFF111214) {
            return c;
        }
        return COLOR_CANCEL_FILL;
    }

    // 7. Home bottom gesture bar (y: 1410..1416)
    if y >= 1410 && y <= 1416 && x >= 260 && x <= 460 {
        return 0x55FFFFFF;
    }

    // Smooth subtle vertical backdrop gradient
    let t = (y.saturating_sub(48) * 255) / (1440 - 48);
    let r1 = (COLOR_BG_TOP >> 16) & 0xFF;
    let g1 = (COLOR_BG_TOP >> 8) & 0xFF;
    let b1 = COLOR_BG_TOP & 0xFF;
    let r2 = (COLOR_BG_BOT >> 16) & 0xFF;
    let g2 = (COLOR_BG_BOT >> 8) & 0xFF;
    let b2 = COLOR_BG_BOT & 0xFF;
    let r = r1 + ((r2 as i32 - r1 as i32) * t as i32 / 255) as u32;
    let g = g1 + ((g2 as i32 - g1 as i32) * t as i32 / 255) as u32;
    let b = b1 + ((b2 as i32 - b1 as i32) * t as i32 / 255) as u32;

    0xFF000000 | (r << 16) | (g << 8) | b
}
