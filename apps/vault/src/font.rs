//! Typography helpers over the InterOS design face (`interos_std::design::font`).
//!
//! Every string is measured before it is drawn. A run that does not fit the box
//! the layout reserved for it is truncated with an ellipsis on a character
//! boundary, and `ink` only ever tests pixels inside that box, so text can never
//! be clipped mid-glyph, never spill into a neighbouring run and never overlap
//! another line.

use interos_std::design::font as face;

/// Re-export of the SDK word-wrapping helper (breaks on spaces, never mid-word).
#[allow(unused_imports)]
pub use interos_std::design::font::wrap_text;

/// Maximum bytes a single fitted run may occupy. Every source run in this app
/// is shorter than this.
pub const RUN_CAP: usize = 96;

/// Measured width of `text` at `scale` in the given weight.
#[inline]
pub fn width(text: &[u8], scale: usize, bold: bool) -> usize {
    if bold {
        face::measure_bold(text, scale)
    } else {
        face::measure(text, scale)
    }
}

/// Copies `text` into `out`, appending "..." when it does not fit `max_w` at
/// `scale`. Truncation only happens between characters, so a glyph is never
/// sliced. Returns the number of bytes written, or 0 when not even the
/// ellipsis fits (the caller then draws nothing rather than a broken run).
pub fn fit(text: &[u8], scale: usize, max_w: usize, bold: bool, out: &mut [u8]) -> usize {
    if text.is_empty() || out.is_empty() || scale == 0 {
        return 0;
    }
    if width(text, scale, bold) <= max_w {
        let n = text.len().min(out.len());
        out[..n].copy_from_slice(&text[..n]);
        return n;
    }
    const DOTS: &[u8] = b"...";
    let dots = width(DOTS, scale, bold);
    if dots > max_w || out.len() < 3 {
        return 0;
    }
    let budget = max_w - dots;
    let mut used = 0usize;
    let mut taken = 0usize;
    for (i, &c) in text.iter().enumerate() {
        let adv = face::advance_of(c, bold) * scale;
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
/// this to right-align a run without guessing the pixel width of its glyphs.
#[inline]
pub fn fit_width(text: &[u8], scale: usize, max_w: usize, bold: bool) -> usize {
    let mut buf = [0u8; RUN_CAP];
    let n = fit(text, scale, max_w, bold, &mut buf);
    width(&buf[..n], scale, bold)
}

/// Anti-aliased run draw. Only tests pixels inside the run's own line box and
/// fits the bytes to `max_w` before touching the glyph table, so a run can
/// never paint outside the box the layout reserved for it. `bg` is the surface
/// under the text so the AA blend is correct. Returns true when `(x, y)`
/// took ink from this run.
///
/// WHY the outline face comes first: the header title is scale 2, so on the
/// bitmap face it is a 16x24 cell magnified by an integer -- a soft ramp where
/// an edge belongs. `outline_text::place_run` rasterises the real outline at the
/// em that matches the cell's cap height (17 rows at scale 1, so
/// 17*N*2048/1456 px per em), and answers for the pixel itself. When it
/// declines -- a run wider than the coverage buffer, a glyph whose ink would
/// leave this caller's window, or a weight this app draws bold -- the bitmap
/// face below draws the run exactly as it did before, so a refusal costs the
/// edge quality and never the text. The header title is drawn `bold`, and the
/// shipped outline table is Roboto Condensed REGULAR, so today that is what
/// declines; see `outline_text`'s module header.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge.
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
    bold: bool,
) -> bool {
    if scale == 0 {
        return false;
    }
    let h = face::line_height(scale);
    if y < y0 || y >= y0 + h || x < x0 || x >= x0 + max_w {
        return false;
    }
    let mut buf = [0u8; RUN_CAP];
    let n = fit(text, scale, max_w, bold, &mut buf);
    if n == 0 {
        return false;
    }
    if let Some(run) = crate::outline_text::place_run(x0, y0, scale, &buf[..n], Some(max_w), bold) {
        return run.pixel(x, y, color, bg).is_some();
    }
    face::test_text_aa(x, y, x0, y0, scale, &buf[..n], color, bg, bold).is_some()
}

/// Right-aligned variant of `ink`: identical guarantees, the run ends at `x1`.
#[inline]
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
    bold: bool,
) -> bool {
    let w = fit_width(text, scale, max_w, bold);
    let x0 = x1.saturating_sub(w);
    ink(x, y, x0, y0, scale, text, max_w, color, bg, bold)
}

/// Centred variant of `ink` inside `[x0, x0 + max_w)`.
#[inline]
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
    bold: bool,
) -> bool {
    let w = fit_width(text, scale, max_w, bold);
    let cx = x0 + max_w.saturating_sub(w) / 2;
    ink(x, y, cx, y0, scale, text, max_w, color, bg, bold)
}
