//! Credential entry surface for unlocking the active user.
//!
//! The lock screen never transitions directly to Home. Unlocking is an
//! authenticated kernel operation; this surface only collects a short numeric
//! PIN and passes it to the user-account syscall.

use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::theme::Theme;

const BG: u32 = 0xFF111214;
const PANEL: u32 = 0xFF25282A;
const BORDER: u32 = 0xFF35383D;
const TEXT: u32 = 0xFFF5F6F7;
const MUTED: u32 = 0xFF9AA0A6;
const ACCENT: u32 = 0xFF38BDF8;
const DANGER: u32 = 0xFFF87171;

const KEY_W: usize = 140;
const KEY_H: usize = 104;
const KEY_GAP: usize = 24;
const KEY_X: usize = 130;
const KEY_Y: usize = 520;

#[inline]
fn round_contains(rect: &Rect, radius: usize, x: usize, y: usize) -> bool {
    if !rect.contains(x, y) { return false; }
    let rx = radius.min(rect.w / 2).min(rect.h / 2);
    let left = rect.x + rx;
    let right = rect.x + rect.w.saturating_sub(rx);
    let top = rect.y + rx;
    let bottom = rect.y + rect.h.saturating_sub(rx);
    if x >= left && x < right || y >= top && y < bottom { return true; }
    let cx = if x < left { left } else { right.saturating_sub(1) };
    let cy = if y < top { top } else { bottom.saturating_sub(1) };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    dx * dx + dy * dy <= (rx as i32) * (rx as i32)
}

#[inline]
fn key_rect(col: usize, row: usize) -> Rect {
    Rect::new(
        KEY_X + col * (KEY_W + KEY_GAP),
        KEY_Y + row * (KEY_H + KEY_GAP),
        KEY_W,
        KEY_H,
    )
}

#[inline]
fn key_digit(col: usize, row: usize) -> u8 {
    match (row, col) {
        (0, 0) => b'1', (0, 1) => b'2', (0, 2) => b'3',
        (1, 0) => b'4', (1, 1) => b'5', (1, 2) => b'6',
        (2, 0) => b'7', (2, 1) => b'8', (2, 2) => b'9',
        _ => b'0',
    }
}

pub fn get_pin_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let _theme = Theme::DARK;
    if y < 48 {
        if state.clock_source_ok {
            if let Some(c) = test_text_aa(x, y, 48, 14, 1, &state.clock_time, TEXT, BG) {
                return c;
            }
        }
        return BG;
    }

    if let Some(c) = test_text_aa(x, y, 48, 150, 3, b"Enter PIN", TEXT, BG) {
        return c;
    }

    let dot_y = 300usize;
    let dot_start = 286usize;
    for i in 0..8 {
        let cx = dot_start + i * 22;
        let active = i < state.pin_len as usize;
        let dx = x as i32 - cx as i32;
        let dy = y as i32 - dot_y as i32;
        if dx * dx + dy * dy <= 36 {
            return if active { ACCENT } else { BORDER };
        }
    }

    if state.pin_error != 0 {
        let msg = if state.pin_error == 2 { b"No credential configured".as_slice() } else { b"Incorrect PIN".as_slice() };
        if let Some(c) = test_text_aa(x, y, 250, 360, 1, msg, DANGER, BG) {
            return c;
        }
    } else if let Some(c) = test_text_aa(x, y, 250, 360, 1, b"Enter your device PIN", MUTED, BG) {
        return c;
    }

    for row in 0..3 {
        for col in 0..3 {
            let rect = key_rect(col, row);
            if round_contains(&rect, 18, x, y) {
                if let Some(c) = test_text_aa(
                    x, y, rect.x + 61, rect.y + 36, 2,
                    &[key_digit(col, row)], TEXT, PANEL
                ) { return c; }
                if x == rect.x || y == rect.y || x + 1 == rect.x + rect.width || y + 1 == rect.y + rect.height {
                    return BORDER;
                }
                return PANEL;
            }
        }
    }

    let zero = key_rect(1, 3);
    if round_contains(&zero, 18, x, y) {
        if let Some(c) = test_text_aa(x, y, zero.x + 61, zero.y + 36, 2, b"0", TEXT, PANEL) { return c; }
        return PANEL;
    }

    let back = Rect::new(490, 1036, 100, 90);
    if round_contains(&back, 18, x, y) {
        if let Some(c) = test_text_aa(x, y, 524, 1070, 1, b"DEL", MUTED, PANEL) { return c; }
        return PANEL;
    }

    let enter = Rect::new(130, 1036, 300, 90);
    if round_contains(&enter, 18, x, y) {
        if let Some(c) = test_text_aa(x, y, 238, 1070, 1, b"UNLOCK", TEXT, ACCENT) { return c; }
        return ACCENT;
    }

    BG
}
