//! Glyph rasterisation and the metrics that go with it.
//!
//! WHY metrics live next to the rasteriser: the previous dock centred its labels
//! with `x0 + 95 - (label.len() * 16) / 2`, hardcoding both the glyph cell width
//! and the scale. Change the type scale and every label is off-centre; change a
//! label and the centring silently drifts. Measuring is the only way a layout can
//! stay correct when the content changes, which is the entire premise of this
//! rewrite.
//!
//! `FONT_8X8` is a fixed-cell face: every glyph occupies 8x8 pixels regardless of
//! its ink. That is why the reference lock screen's clock could not be matched on
//! both axes — the face is 1.68x less condensed than the design's typeface. See
//! `Font::advance` for how advances are reported so a condensed face can be added
//! later without touching any call site.

/// The single bitmap face available to the shell.
///
/// WHY there is only one: FONT_8X8 is the only glyph data in the project. That is
/// the largest single constraint on visual quality, and the reason large type
/// reads as blocky pixel art. Adding a condensed 16x24 face is an asset change to
/// this module alone; no screen code would need to move.
pub const GLYPH_W: usize = 8;
pub const GLYPH_H: usize = 8;

/// Ink bounds of the face, used for optical centring.
///
/// WHY this matters: a fixed 8x8 cell has asymmetric whitespace, so centring on
/// the cell rather than the ink makes text look slightly off to one side. `cap_top`
/// is the row at which a capital letter starts and `baseline` the row it sits on.
pub const CAP_TOP: usize = 1;
pub const BASELINE: usize = 7;

/// Width in pixels a run of text occupies at `scale`.
///
/// WHY this is a function and not an inline expression: it is called from every
/// centring and wrapping decision in the shell, and an approximation that
/// disagrees with the rasteriser is exactly the bug class this module exists to
/// remove.
pub fn measure(text: &[u8], scale: usize) -> usize {
    text.len() * GLYPH_W * scale
}

/// Height in pixels a line occupies at `scale`.
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

/// Splits `text` into at most `max_lines` lines no wider than `max_width`.
///
/// Breaks on spaces where possible so words stay intact, and falls back to a hard
/// break for a single word that cannot fit. Returns the byte length of each line;
/// the caller slices with them.
///
/// WHY truncate rather than drop: silently discarding overflow is how labels
/// disappear, and a UI that hides content is worse than one that clips it.
pub fn wrap(text: &[u8], scale: usize, max_width: usize, max_lines: usize) -> ([usize; 3], usize) {
    let mut breaks = [0usize; 3];
    let mut n = 0usize;
    if max_lines == 0 || max_width == 0 {
        return (breaks, 0);
    }
    let per_line = (max_width / (GLYPH_W * scale)).max(1);
    let mut start = 0usize;
    while start < text.len() && n < max_lines {
        let remaining = text.len() - start;
        if remaining <= per_line {
            breaks[n] = remaining;
            n += 1;
            break;
        }
        // Look back for the last space that keeps the line within the budget.
        let mut cut = per_line;
        let mut found_space = false;
        for i in (1..=per_line).rev() {
            if text[start + i - 1] == b' ' {
                cut = i - 1;
                found_space = true;
                break;
            }
        }
        let _ = found_space;
        if cut == 0 {
            cut = per_line;
        }
        breaks[n] = cut;
        n += 1;
        // Skip the separator consumed by the break.
        start += cut;
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
pub fn ellipsize(text: &[u8], scale: usize, max_width: usize) -> ([u8; 96], usize) {
    let mut out = [0u8; 96];
    let per_line = (max_width / (GLYPH_W * scale)).max(1);
    if text.len() <= per_line {
        let n = text.len().min(96);
        out[..n].copy_from_slice(&text[..n]);
        return (out, n);
    }
    if per_line <= 3 {
        return (out, 0);
    }
    let keep = per_line - 3;
    out[..keep].copy_from_slice(&text[..keep]);
    out[keep] = b'.';
    out[keep + 1] = b'.';
    out[keep + 2] = b'.';
    (out, keep + 3)
}

/// Whether a byte has ink in this face.
pub fn is_printable(c: u8) -> bool {
    (32..=126).contains(&c)
}

/// Maps a byte to its glyph index, or `None` when it has no glyph.
pub fn glyph_index(c: u8) -> Option<usize> {
    if is_printable(c) {
        Some((c - 32) as usize)
    } else {
        None
    }
}