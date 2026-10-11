//! Typography over the shared InterOS design face (`interos_std::design::font`).
//!
//! WHY THIS REPLACED THE OLD MODULE: this app used to carry its own 8x8 face
//! with a fixed 8px-per-character advance and no anti-aliasing. That advance is
//! what put a 51-character status line 154px and a 53-character footer note
//! 172px off a 720px surface: the runs were laid out against a cell size the
//! screen never had. Every other native app measures its runs against the shared
//! 16x24 proportional face; this one now does too.
//!
//! Every run is measured before it is drawn and is fitted to the box the layout
//! reserved for it, so a run can never leave its box, is never cut mid-glyph and
//! is anti-aliased against the surface it is actually painted on.

use interos_std::design::font as face;

/// Re-export of the SDK word-wrapping helper (breaks on spaces, never mid-word).
pub use interos_std::design::font::wrap_text;

/// Maximum bytes a single fitted run may occupy. The largest buffer any call
/// site hands in is the 96-byte kernel `error` payload; every literal run and
/// every assembled verdict is shorter.
pub const RUN_CAP: usize = 96;

/// Measured width of `text` at `scale`.
#[inline]
#[allow(dead_code)]
pub fn width(text: &[u8], scale: usize) -> usize {
    face::measure(text, scale)
}

/// Height of one line box at `scale` (24 source rows, so `24 * scale`).
#[inline]
#[allow(dead_code)]
pub fn line_height(scale: usize) -> usize {
    face::line_height(scale)
}

/// Copies `text` into `out`, appending "..." when it does not fit `max_w` at
/// `scale`. Truncation only happens between characters, so a glyph is never
/// sliced. Returns the number of bytes written, or 0 when not even the ellipsis
/// fits (the caller then draws nothing rather than a broken run).
pub fn fit(text: &[u8], scale: usize, max_w: usize, out: &mut [u8]) -> usize {
    if text.is_empty() || out.is_empty() || scale == 0 {
        return 0;
    }
    if face::measure(text, scale) <= max_w {
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        return n;
    }
    const DOTS: &[u8] = b"...";
    let dots = face::measure(DOTS, scale);
    if dots > max_w || out.len() < 3 {
        return 0;
    }
    let budget = max_w - dots;
    let mut used = 0usize;
    let mut taken = 0usize;
    for (i, &c) in text.iter().enumerate() {
        let adv = face::advance_of(c, false) * scale;
        if used + adv > budget || taken + 3 > out.len() {
            break;
        }
        used += adv;
        taken = i + 1;
    }
    out[..taken].copy_from_slice(&text[..taken]);
    out[taken..taken + 3].copy_from_slice(DOTS);
    taken + 3
}

/// Width the run will really occupy once `fit` has been applied. Callers use
/// this to place the next column without guessing a pixel width.
#[inline]
pub fn fit_width(text: &[u8], scale: usize, max_w: usize) -> usize {
    let mut buf = [0u8; RUN_CAP];
    let n = fit(text, scale, max_w, &mut buf);
    face::measure(&buf[..n], scale)
}

/// Anti-aliased run draw. Only tests pixels inside the run's own line box and
/// fits the bytes to `max_w` before touching the glyph table, so a run can never
/// paint outside the box the layout reserved for it. `bg` is the surface under
/// the text so the anti-aliasing blend is correct. Returns true when `(x, y)`
/// took ink from this run.
#[inline]
pub fn ink(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> bool {
    if scale == 0 {
        return false;
    }
    let h = face::line_height(scale);
    if y < y0 || y >= y0 + h || x < x0 || x >= x0 + max_w {
        return false;
    }
    let mut buf = [0u8; RUN_CAP];
    let n = fit(text, scale, max_w, &mut buf);
    if n == 0 {
        return false;
    }
    face::test_text_aa(x, y, x0, y0, scale, &buf[..n], color, bg, false).is_some()
}

/// Right-aligned variant of `ink`: identical guarantees, the run ends at `x1`.
#[inline]
#[allow(dead_code)]
pub fn ink_right(
    x: usize,
    y: usize,
    x1: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> bool {
    let w = fit_width(text, scale, max_w);
    let x0 = x1.saturating_sub(w);
    ink(x, y, x0, y0, scale, text, max_w, color, bg)
}

/// Centred variant of `ink` inside `[x0, x0 + max_w)`.
#[inline]
#[allow(dead_code)]
pub fn ink_center(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
    bg: u32,
) -> bool {
    let w = fit_width(text, scale, max_w);
    let cx = x0 + max_w.saturating_sub(w) / 2;
    ink(x, y, cx, y0, scale, text, max_w, color, bg)
}