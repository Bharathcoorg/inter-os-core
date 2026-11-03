// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Drawing primitives the launcher's screens share.
//!
//! WHY this module exists: the helpers below were living in the System Space
//! screen, and three other screens reached into that screen's module to borrow
//! them. That coupling meant a screen could not be removed without three
//! unrelated screens failing to compile, and it hid the fact that these are
//! general drawing primitives rather than anything to do with settings.
//! They are here now, which is where they belong.

#![allow(dead_code)]

use crate::test_text_aa;
use launcher::layout::Rect;
use launcher::text;
use launcher::theme::Theme;

/// Maximum value buffer capacity.
pub(super) const VALUE_MAX: usize = 48;

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
    let h = run_line_box(scale);
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
    let h = run_line_box(scale);
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
    let h = run_line_box(scale);
    let y0 = rect.y + rect.h.saturating_sub(h) / 2;
    if y < y0 || y >= y0 + h {
        return None;
    }
    let w = text::measure(run, scale);
    if w > rect.w {
        return None;
    }
    let cx = (rect.w.saturating_sub(w)) / 2;
    let placed = Rect::new(rect.x + cx, rect.y, w, rect.h);
    run_left(x, y, &placed, scale, run, color)
}

/// The height of the line box the painter actually inks for a run at `scale`.
///
/// WHY the two faces are named apart: a run at scale two and above is drawn
/// with the outline face, whose box is the ladder's ascent plus descent; the
/// bitmap face at scale one draws its whole `GLYPH_H` cell. The old helpers
/// measured the run's width with `scale * 24`, an em roughly double the ladder
/// one the outline is placed at, so a run that fitted its card measured as
/// overflowing and was dropped, and centring put the rest off to one side.
/// [`text::measure`] walks the same advances the rasteriser steps by, so the
/// bound and the ink cannot drift apart.
fn run_line_box(scale: usize) -> usize {
    if scale >= 2 {
        launcher::font::line_height(scale)
    } else {
        text::line_height(scale)
    }
}

/// A settings value or absence reason.
pub(super) struct Reading {
    bytes: [u8; VALUE_MAX],
    len: usize,
    available: bool,
}

impl Reading {
    /// A value the platform actually reported.
    pub(super) fn of(run: &[u8]) -> Reading {
        let mut bytes = [0u8; VALUE_MAX];
        let len = run.len().min(VALUE_MAX);
        bytes[..len].copy_from_slice(&run[..len]);
        Reading {
            bytes,
            len,
            available: true,
        }
    }

    /// A value the platform did not report, and the reason it did not.
    pub(super) fn unavailable(reason: &[u8]) -> Reading {
        let mut reading = Reading::of(reason);
        reading.available = false;
        reading
    }

    /// The bytes of the reading.
    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// The fill and the ink this reading is drawn with.
    pub(super) fn skin(&self, theme: &Theme) -> (u32, u32) {
        if self.available {
            (theme.surface_container, theme.on_surface)
        } else {
            (theme.surface_container_low, theme.on_surface_variant)
        }
    }
}

/// Appends `run` to a value buffer, stopping at its capacity.
pub(super) fn push(buf: &mut [u8; VALUE_MAX], at: &mut usize, run: &[u8]) {
    for b in run {
        if *at >= VALUE_MAX {
            return;
        }
        buf[*at] = *b;
        *at += 1;
    }
}

/// Formats `n` as decimal digits.
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

/// Formats `n` followed by `suffix`.
pub(super) fn counted(n: usize, suffix: &[u8]) -> ([u8; VALUE_MAX], usize) {
    let (mut buf, mut at) = decimal(n);
    push(&mut buf, &mut at, suffix);
    (buf, at)
}
