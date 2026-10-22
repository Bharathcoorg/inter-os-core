//! Glyph metrics, and the rasterisation helper that agrees with them.
//!
//! WHY metrics live next to the rasteriser: the previous dock centred its labels
//! with `x0 + 95 - (label.len() * 16) / 2`, hardcoding both the glyph cell width
//! and the scale. Change the type scale and every label is off-centre; change a
//! label and the centring silently drifts. Measuring is the only way a layout can
//! stay correct when the content changes, which is the entire premise of this
//! rewrite.
//!
//! WHY `measure` is now a sum of advances and not `len * GLYPH_W * scale`: the
//! face in [`crate::font`] is proportional. `i` advances 6 px, `m` 13, `W` 16,
//! and the figures are tabular at 10. A fixed-cell formula would report the
//! width of `iiii` as four cells wide and lay every label out for a face this
//! shell no longer has — the exact bug class this module exists to prevent.
//! `measure` is the sum of the advances the rasteriser actually steps by, so the
//! two cannot drift apart; `font::run_pixel` and `measure` are both asserted to
//! agree in `font.rs`'s tests.
//!
//! WHY a proportional face at all: `FONT_8X8` was a fixed 8x8 cell, so it was
//! 1.68x *less* condensed than the reference design's typeface, and a fixed cell
//! cannot be condensed — only shrunk. That mismatch is why the reference lock
//! screen's clock could not be matched on both axes. Condensing required real
//! metrics, and real metrics required giving up the fixed cell.

pub use crate::font::{BASELINE, CAP_TOP, GLYPH_H, GLYPH_W};

use crate::font;

/// Advance of one byte in device pixels at `scale`, ignoring weight.
///
/// WHY a byte with no glyph still advances: it has to, or `measure` and the
/// rasteriser disagree for any string containing one and every centred label
/// containing one drifts. Both paths read `font::FALLBACK_ADVANCE`.
///
/// WHY this is the advance the run is *drawn* with and not the bitmap cell's:
/// a run at scale two and above is drawn with the outline face, and the
/// outline's advance is narrower than the bitmap cell's. Measuring a run with
/// the bitmap's advance and drawing it with the outline's put every label
/// to the left of the centre of its tile by the difference -- which is what
/// the home deck's captions looked like.
#[inline]
pub fn advance(c: u8, scale: usize) -> usize {
    if scale >= 2 {
        let em = crate::outline_text::em_for_scale(scale);
        let outline = interos_std::design::typeface::text_advance_px(&[c], em) as usize;
        if outline > 0 {
            return outline;
        }
        // WHY the fallback is the bitmap's advance and not zero: a byte
        // the outline face has no glyph for would advance by nothing, and
        // the wrap and the ellipsis loops advance a cursor that never
        // moves. A label with one unmapped byte in it hung the shell's
        // home screen.
    }
    font::advance_of(c, false) * scale
}

/// Width in pixels a run of text occupies at `scale`.
///
/// This is the true advance width: the sum of each glyph's own advance, times
/// `scale`. It is what a centring, wrapping or truncation decision must be based
/// on, and it is what the rasteriser below steps by.
pub fn measure(text: &[u8], scale: usize) -> usize {
    let mut w = 0usize;
    for &c in text {
        w += advance(c, scale);
    }
    w
}

/// Height in pixels a line occupies at `scale`.
///
/// WHY this is the whole cell and not the cap height: the cell is what the
/// rasteriser can touch, so it is what a caller must reserve. Screens that want
/// to centre a line optically should use [`CAP_TOP`] and [`BASELINE`] to bias it
/// within that box.
pub fn line_height(scale: usize) -> usize {
    GLYPH_H * scale
}

/// Left edge that centres `text` horizontally within `available`.
///
/// Returns 0 rather than underflowing when the text is wider than the space,
/// which is what lets a caller clamp rather than panic.
pub fn center_x(text: &[u8], scale: usize, available: usize) -> usize {
    let w = measure(text, scale);
    if w >= available {
        0
    } else {
        (available - w) / 2
    }
}

/// Turns a reverse-DNS package id into a title a person can read.
///
/// WHY this is a fallback and not a name: the package registry carries a display
/// label for every package the shell has catalogued, and that label is always
/// preferred. This exists for the case where it is not available -- a task
/// restored from the window list for a package the registry no longer describes.
/// That case used to render the id itself, so a card read `org.interos.card_demo`
/// where a person expects `Card Demo`.
///
/// WHY this is not fabrication: nothing is looked up or guessed. The vendor
/// segments are dropped, `_`, `-` and `.` become word breaks, and each word is
/// capitalised. The bytes are the same bytes the id already had.
///
/// WHY the LAST segment and not the first: a reverse-DNS id is
/// `<vendor>...<app>`, and the app name is the final segment. `org.interos` is a
/// vendor, `card_demo` is the app. Stripping only the first segment leaves
/// `interos.card_demo`, which reads as a product line nobody ships.
pub fn humanise_package_id<'a>(id: &[u8], buf: &'a mut [u8]) -> &'a [u8] {
    let mut start = 0usize;
    if let Some(last) = id.iter().rposition(|b| *b == b'.') {
        // Only take the tail when there IS one: `org` is an app called "org",
        // and rendering it as an empty label would be worse than showing it.
        if last + 1 < id.len() {
            start = last + 1;
        }
    }

    let mut at = 0usize;
    let mut capitalise = true;
    for &b in &id[start..] {
        if b == b'_' || b == b'-' || b == b'.' {
            if at > 0 && buf[at - 1] != b' ' {
                if at >= buf.len() {
                    break;
                }
                buf[at] = b' ';
                at += 1;
            }
            capitalise = true;
            continue;
        }
        // A digit ends the word, so `card2demo` reads "Card2demo" and not
        // "Card2D..." -- there is no word to capitalise after a figure.
        let c = if capitalise {
            capitalise = false;
            b.to_ascii_uppercase()
        } else {
            b
        };
        if at >= buf.len() {
            break;
        }
        buf[at] = c;
        at += 1;
    }

    while at > 0 && buf[at - 1] == b' ' {
        at -= 1;
    }
    &buf[..at]
}

/// Splits `text` into at most `max_lines` lines no wider than `max_width`.
///
/// Returns the byte length of each line; the caller slices with them.
///
/// WHY widths and not character counts: with proportional advances a line of
/// `iiiii` and a line of `WWWWW` hold very different numbers of characters, so a
/// character budget is either a lie or wastes most of the line. Each line here
/// is measured with [`measure`], the same function that positions it.
///
/// Breaks on a space where one is available so words stay intact, and falls back
/// to a hard break for a word too long to fit. Why truncate rather than drop:
/// silently discarding overflow is how labels disappear, and a UI that hides
/// content is worse than one that clips it.
///
/// WHY the line count is clamped to the array: the return type is a fixed
/// `[usize; 3]`, so a caller asking for more lines than that cannot be given
/// them, and writing past the array would be worse than returning fewer.
pub fn wrap(text: &[u8], scale: usize, max_width: usize, max_lines: usize) -> ([usize; 3], usize) {
    let mut breaks = [0usize; 3];
    let budget = max_lines.min(breaks.len());
    let mut n = 0usize;
    if budget == 0 || max_width == 0 || scale == 0 {
        return (breaks, 0);
    }
    let mut start = 0usize;
    while start < text.len() && n < budget {
        // Widest run from `start` that fits, in bytes.
        let mut end = start;
        let mut w = 0usize;
        while end < text.len() {
            let a = advance(text[end], scale);
            if end > start && w + a > max_width {
                break;
            }
            w += a;
            end += 1;
        }
        // Prefer the last space inside the run so the word stays whole.
        let mut cut = end;
        if end < text.len() {
            let mut found = None;
            for i in (start..end).rev() {
                if text[i] == b' ' {
                    found = Some(i);
                    break;
                }
            }
            if let Some(i) = found {
                if i > start {
                    cut = i;
                }
            }
        }
        breaks[n] = cut - start;
        n += 1;
        start = cut;
        // Skip the separator consumed by the break.
        if start < text.len() && text[start] == b' ' {
            start += 1;
        }
    }
    (breaks, n)
}

/// Truncates `text` to `max_width`, appending an ellipsis when it had to cut.
///
/// WHY an ellipsis and not a hard cut: a clipped word reads as a rendering fault,
/// whereas an ellipsis reads as intentional.
///
/// WHY it reserves room for the ellipsis before choosing the prefix: a string
/// truncated to the full width and then given an ellipsis is wider than the
/// space it was fitted to, which is how a label ends up one ellipsis past the
/// edge of its card. Returns nothing when even `...` will not fit.
pub fn ellipsize(text: &[u8], scale: usize, max_width: usize) -> ([u8; 96], usize) {
    let mut out = [0u8; 96];
    if scale == 0 || max_width == 0 {
        return (out, 0);
    }
    if measure(text, scale) <= max_width {
        let n = text.len().min(96);
        out[..n].copy_from_slice(&text[..n]);
        return (out, n);
    }
    let dots = measure(b"...", scale);
    if dots > max_width {
        return (out, 0);
    }
    let budget = max_width - dots;
    let mut keep = 0usize;
    let mut w = 0usize;
    for (i, &c) in text.iter().enumerate() {
        if keep >= out.len() - 3 {
            break;
        }
        let a = advance(c, scale);
        if w + a > budget {
            break;
        }
        w += a;
        keep = i + 1;
    }
    out[..keep].copy_from_slice(&text[..keep]);
    out[keep..keep + 3].copy_from_slice(b"...");
    (out, keep + 3)
}

/// Whether a byte has ink in this face.
pub fn is_printable(c: u8) -> bool {
    (32..=126).contains(&c)
}

/// A label placed once for a frame.
///
/// WHY this exists: the per-pixel painter is called once for every pixel of the
/// surface, and the old helpers re-placed the whole run for every one of them.
/// Placing a run costs a glyph lookup per character, so a screen with ten
/// labels was doing ten million glyph lookups per frame. A [`Label`] is placed
/// once, when the frame is planned, and then answers "do you ink this pixel?"
/// for the rest of the frame.
///
/// WHY the outline face is preferred at scale 2 and above: the bitmap face is
/// the fallback for the smallest step, and the platform's own screens draw
/// their body copy with the outline face, so a label drawn here has to be the
/// same letterforms or it reads as a different app.
#[derive(Clone, Copy)]
pub struct Label {
    x: usize,
    top: usize,
    w: usize,
    h: usize,
    scale: usize,
    len: usize,
    ink: u32,
    bytes: [u8; LABEL_CAP],
    outline: Option<crate::outline_text::OutlineRun>,
}

/// The longest run a [`Label`] can hold.
pub const LABEL_CAP: usize = 64;

impl Label {
    /// A label that never inks anything.
    pub const EMPTY: Label = Label {
        x: 0,
        top: 0,
        w: 0,
        h: 0,
        scale: 1,
        len: 0,
        ink: 0,
        bytes: [0; LABEL_CAP],
        outline: None,
    };

    /// Places `text` with its line box's top-left at (`x`, `y`).
    pub fn at(x: usize, y: usize, scale: usize, text: &[u8], ink: u32) -> Label {
        let mut bytes = [0u8; LABEL_CAP];
        let len = text.len().min(LABEL_CAP);
        bytes[..len].copy_from_slice(&text[..len]);
        let run = &bytes[..len];
        let em = crate::outline_text::em_for_scale(scale);
        let (ascent, _descent) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + font::ascent_for_scale(scale)).max(ascent as usize);
        let outline = if scale >= 2 {
            crate::outline_text::OutlineRun::place(x, baseline, em, run)
        } else {
            None
        };
        let (bx, by, bw, bh) = outline
            .map(|r| r.bounds())
            .unwrap_or((x, y, measure(run, scale), line_height(scale)));
        Label {
            x: bx,
            top: by,
            w: bw,
            h: bh,
            scale,
            len,
            ink,
            bytes,
            outline,
        }
    }

    /// The label's width in pixels.
    pub fn width(&self) -> usize {
        self.w
    }

    /// The colour of (`px`, `py`) if this label inks it, blended over `bg`.
    pub fn pixel(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if px < self.x || px >= self.x + self.w || py < self.top || py >= self.top + self.h {
            return None;
        }
        if let Some(ref outline) = self.outline {
            return outline.pixel(px, py, self.ink, bg);
        }
        font::test_text_aa(
            px,
            py,
            self.x,
            self.top,
            self.scale,
            &self.bytes[..self.len],
            self.ink,
            bg,
            false,
        )
    }
}

/// Maps a byte to its glyph index, or `None` when it has no glyph.
pub fn glyph_index(c: u8) -> Option<usize> {
    if is_printable(c) {
        Some((c - 32) as usize)
    } else {
        None
    }
}

/// Rasterises one device pixel of a run of text, returning `color` where the
/// glyph has ink and `None` where it does not.
///
/// THIS is what `main.rs`'s `test_text` must call. The version it replaces
/// derived the glyph from `(x - x0) / char_w`, which is only correct when every
/// character is the same width; with proportional advances the pen has to be
/// walked, and this walks it with the same [`advance`] the metrics use.
///
/// The argument order and meaning are identical to the function it replaces, so
/// no call site changes: `x0, y0` is the top-left of the first glyph cell, and
/// the run advances left to right by each glyph's own advance.
#[inline(always)]
pub fn rasterize_pixel(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    match font::run_pixel(x, y, x0, y0, scale, text, false) {
        Some(_) => Some(color),
        None => None,
    }
}

/// [`rasterize_pixel`] in the bold weight.
///
/// Not used by the shell yet: it is here so a screen that wants a heavier label
/// can have one without a second rasteriser, and so the bold data in
/// [`font::FONT_BOLD`] is reachable rather than dead weight in the binary.
#[inline(always)]
pub fn rasterize_pixel_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    match font::run_pixel(x, y, x0, y0, scale, text, true) {
        Some(_) => Some(color),
        None => None,
    }
}