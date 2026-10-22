//! Credential entry surface for unlocking the active user.
//!
//! The lock screen never transitions directly to Home. Unlocking is an
//! authenticated kernel operation; this surface only collects a short numeric
//! PIN and passes it to the user-account syscall.

use crate::{test_text_aa, LauncherState};
use launcher::layout::Rect;
use launcher::theme::{Theme, SCREEN_WIDTH};

/// How many entry dots the pad draws: the longest PIN this surface accepts, which
/// is the `pin_buffer`'s own length in `main.rs`.
const DOT_COUNT: u8 = 8;


const KEY_W: usize = 140;
const KEY_H: usize = 104;
const KEY_GAP: usize = 24;
const KEY_X: usize = 130;
const KEY_Y: usize = 520;

/// Row the entry dots sit on.
///
/// WHY named: the dots and the band the shell repaints when one fills or empties
/// are the same number, and they were a literal inside the painter and a second
/// literal inside the shell when the whole surface was repainted anyway.
const DOT_Y: usize = 300;
/// Radius of an entry dot. The painter's gate is `dx*dx + dy*dy <= R * R`.
const DOT_R: usize = 6;
/// Row the status line under the dots starts on.
const STATUS_Y: usize = 360;
/// Row the prompt starts on.
const PROMPT_Y: usize = 150;

/// Rows the entry dots occupy.
///
/// WHY read back from here: `get_pin_color` gates a dot's ink on the same radius
/// this band is derived from, so the shell repainting `dots_band()` repaints
/// exactly the rows a digit can fill or empty.
pub fn dots_band() -> (usize, usize) {
    (DOT_Y - DOT_R, DOT_Y + DOT_R + 1)
}

/// Rows the status line under the dots occupy: the error line, the confirmation
/// hint, or the prompt for the entry.
pub fn status_band() -> (usize, usize) {
    (STATUS_Y, STATUS_Y + launcher::text::line_height(1))
}

/// Rows from the prompt down to the bottom of the status line.
///
/// WHY one band for all of it: every change this surface can make without
/// leaving it -- a digit, a backspace, a failed submit, the setup step moving
/// forward -- moves something in this extent and nothing below it. The keypad
/// rows are invariant state.
pub fn entry_band() -> (usize, usize) {
    (PROMPT_Y, STATUS_Y + launcher::text::line_height(1))
}

    /// The colour of (`x`, `y`) when the run at (`x0`, `y0`) on `scale` inks it,
    /// drawn by the outline rasteriser when it can place the run and by the bitmap
    /// face when it cannot.
    ///
    /// WHY the vertical band is a union of both renderers: the outline run's line
    /// box is not the bitmap cell. At the em matching a scale-3 cap the outline's
    /// ascent reaches 75 rows above the run's baseline and its descent 19 rows
    /// below, against the bitmap's 4-rows-above and 4-rows-below (rows 3..23 of a
    /// 24-row cell); measured, the heading's cap ink stops 52 rows above its
    /// baseline and its lowest glyph ends at the baseline. At a scale-1 caption the
    /// figures are 25 and 6, against an ink extent of 18 above and 5 below. A band
    /// measured for the cell alone clips both faces silently, and this surface's
    /// geometry contains the union everywhere it is used: the prompt band is
    /// `y0-15 .. y0+79` against the 294-row gap above the dots, a key's digit band
    /// is `rect.y+26 .. rect.y+89` against a 104-row key, and the status band is
    /// `y0-5 .. y0+26` with the keypad rows 134 below it.
    ///
    /// WHY the caller's own control test still comes first: the keys, the discs and
    /// the status bar keep the hit tests and borders they had, so the wider band can
    /// only add rows the control already covers.
    ///
    /// Why the attribute, and why nothing calls this: every run on this surface is
    /// one or the other of two things the outline face cannot serve correctly. The
    /// keypad digits are BOLD at scale 2, and the shipped outline table is Roboto
    /// Condensed Regular, so routing them through it would de-bold them -- a
    /// different change from the one this makes. The prompt and status lines are
    /// scale 1, where the cell is drawn at its native size and there is no
    /// magnified edge to fix. The funnel is kept because it is the shape any
    /// caller here would need the day either of those changes, and the doc comment
    /// records what stands in the way.
    #[allow(dead_code)]
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
    // Choose the renderer ONCE for the run, not once per pixel. The placement is
    // a pure function of the run's origin, em and bytes, so every pixel of this
    // run reaches the same face. A per-pixel fallback would draw the glyph from
    // both faces at once -- an outline glyph under a hard-edged bitmap ghost.
    if scale >= 1 && bytes.len() <= launcher::outline_text::KEY_MAX {
        if let Some(run) = launcher::outline_text::OutlineRun::place(x0, baseline, em, bytes) {
            return run.pixel(x, y, ink, bg);
        }
    }
    // The bitmap face is the refusal path, not the baseline: a run the rasteriser
    // declines still has to draw as text.
    test_text_aa(x, y, x0, y0, scale, bytes, ink, bg)
}

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
    let theme = Theme::DARK;
    if y < 48 {
        if state.clock_source_ok {
            if let Some(c) = test_text_aa(x, y, 48, 14, 1, &state.clock_time, theme.on_surface, theme.surface) {
                return c;
            }
        }
        return theme.surface;
    }

    // WHY the step is shown: first boot asks for the PIN twice, and a second
    // screen captioned "Set Device PIN" reads as a reset of the first rather
    // than a confirmation of it.
    let heading = if state.pin_setup_mode {
        if state.pin_setup_step == 0 { b"Set Device PIN".as_slice() } else { b"Confirm Device PIN".as_slice() }
    } else {
        b"Enter PIN".as_slice()
    };
    if let Some(c) = test_text_aa(x, y, 48, PROMPT_Y, 3, heading, theme.on_surface, theme.surface) {
        return c;
    }

    let dot_y = DOT_Y;
    // WHY derived and not a literal: `286` was a typed start, and it put the row's
    // centre (between the fourth and fifth dot) at 363 rather than on the surface's
    // own middle, 360 -- the eight dots were drawn three pixels right of centre on
    // every entry attempt. The pitch and the count are both still literals because
    // they are this keypad's own spacing, but the start is now solved for the
    // centre, so a change to either only moves the row, never off-centres it.
    let dot_pitch = 22usize;
    let dot_start = SCREEN_WIDTH / 2 - (DOT_COUNT as usize - 1) * dot_pitch / 2;
    for i in 0..DOT_COUNT as usize {
        let cx = dot_start + i * dot_pitch;
        let active = i < state.pin_len as usize;
        let dx = x as i32 - cx as i32;
        let dy = y as i32 - dot_y as i32;
        if dx * dx + dy * dy <= (DOT_R * DOT_R) as i32 {
            return if active { theme.primary } else { theme.border };
        }
    }

    if state.pin_error != 0 {
        let msg = if state.pin_error == 2 { b"No credential configured".as_slice() } else { b"Incorrect PIN".as_slice() };
        if let Some(c) = test_text_aa(x, y, 250, STATUS_Y, 1, msg, theme.error, theme.surface) {
            return c;
        }
    } else if state.pin_setup_mode && state.pin_setup_step == 1 {
        if let Some(c) = test_text_aa(x, y, 250, STATUS_Y, 1, b"Re-enter the same PIN", theme.on_surface_variant, theme.surface) {
            return c;
        }
    } else if let Some(c) = test_text_aa(x, y, 250, STATUS_Y, 1, if state.pin_setup_mode { b"Choose 4 to 8 digits".as_slice() } else { b"Enter your device PIN".as_slice() }, theme.on_surface_variant, theme.surface) {
        return c;
    }

    for row in 0..3 {
        for col in 0..3 {
            let rect = key_rect(col, row);
            if round_contains(&rect, 18, x, y) {
                if let Some(c) = test_text_aa(
                    x, y, rect.x + 61, rect.y + 36, 2,
                    &[key_digit(col, row)], theme.on_surface, theme.surface_container
                ) { return c; }
                if x == rect.x || y == rect.y || x + 1 == rect.x + rect.w || y + 1 == rect.y + rect.h {
                    return theme.border;
                }
                return theme.surface_container;
            }
        }
    }

    let zero = key_rect(1, 3);
    if round_contains(&zero, 18, x, y) {
        if let Some(c) = test_text_aa(x, y, zero.x + 61, zero.y + 36, 2, b"0", theme.on_surface, theme.surface_container) { return c; }
        return theme.surface_container;
    }

    let back = Rect::new(490, 1036, 100, 90);
    if round_contains(&back, 18, x, y) {
        if let Some(c) = test_text_aa(x, y, 524, 1070, 1, b"DEL", theme.on_surface_variant, theme.surface_container) { return c; }
        return theme.surface_container;
    }

    let enter = Rect::new(130, 1036, 300, 90);
    if round_contains(&enter, 18, x, y) {
        if let Some(c) = test_text_aa(x, y, 238, 1070, 1, b"UNLOCK", theme.on_surface, theme.primary) { return c; }
        return theme.primary;
    }

    theme.surface
}

/// The key a point on the PIN surface presses, or `None` outside every key.
///
/// WHY this lives beside [`key_rect`]: the hit test and the drawing must agree
/// about where a key is, and they used to be two separate geometries - the
/// drawer's own pad table placed its rows at 700/840/980/1120 while this surface
/// draws them at 572/700/828/956. The top row therefore did not hit-test at all
/// and the bottom row answered as the row above it, which is why entering a PIN
/// through the surface could register the wrong digits. Resolving a point
/// against `key_rect` means there is exactly one pad geometry to change.
///
/// `'E'` is the UNLOCK action and `'D'` the DEL action, matching the two buttons
/// this surface draws beneath the keys.
#[allow(dead_code)]
pub fn key_at(x: usize, y: usize) -> Option<char> {
    for row in 0..3 {
        for col in 0..3 {
            let rect = key_rect(col, row);
            if round_contains(&rect, 18, x, y) {
                return Some(key_digit(col, row) as char);
            }
        }
    }
    let zero = key_rect(1, 3);
    if round_contains(&zero, 18, x, y) {
        return Some('0');
    }
    let back = Rect::new(490, 1036, 100, 90);
    if round_contains(&back, 18, x, y) {
        return Some('D');
    }
    let enter = Rect::new(130, 1036, 300, 90);
    if round_contains(&enter, 18, x, y) {
        return Some('E');
    }
    None
}
