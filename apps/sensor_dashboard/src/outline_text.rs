//! Outline type for the sensor dashboard's headings, over the shared rasteriser.
//!
//! WHY this exists alongside the bitmap face: `interos_std::design::font` is a
//! 16x24 cell drawn at an integer multiple, so this app's larger type is that
//! cell magnified. Every heading runs at `HSC` = 2, and a magnified
//! anti-aliased edge reads as a soft ramp rather than an edge -- the staircasing
//! this replaces. No integer multiple of a 24px cell puts a real edge anywhere.
//!
//! `design::typeface` rasterises real outlines at any em, so an edge lands where
//! the outline puts it. THIS IS THE SAME INTEGRATION as
//! `apps/launcher/src/outline_text.rs` -- `place` / `pixel` over a bounded
//! coverage buffer, copied rather than reinvented, with the bitmap face kept as
//! the fallback everywhere else so every refusal costs one run and never drops
//! text.
//!
//! WHY the em is DERIVED here and not copied from the bitmap scale: the bitmap
//! cell inks its cap over rows 3..19 of a 24-row cell (`FONT_COV`'s 'H' covers
//! exactly 17 rows at scale 1), so a bitmap run at scale N has a 17*N-pixel cap.
//! Roboto Condensed's cap height is 1456 of its 2048 em -- measured from the
//! shipped `design::face` outline table, where 'H', 'A', 'N' and 'E' all span
//! 1456 units -- so the em that reproduces the same cap height is
//!
//!   em = round(17 * N * 2048 / 1456)   =>   scale 1 -> 24, 2 -> 48, 3 -> 72.
//!
//! That is a chosen size, not a magnified cell. At em 48 the face's cap is 34px
//! and its stems are 4.3px, against the bitmap cell's 34px cap and 4px regular
//! stems, so an outline run replaces the magnified cell at the same optical
//! size and the same (regular) weight. The rows this app draws at `SC` = 1 keep
//! the bitmap face: at scale 1 the cell is at its native size, so there is no
//! magnified edge to fix.
//!
//! WHY no `#[cfg(test)]` block: this app's bin declares `test = false`, so a
//! test module inside it is never compiled and would rot. The module this
//! mirrors, `apps/launcher/src/outline_text.rs`, carries that coverage in a lib
//! target that `cargo test` does run.
//!
//! WHY `OutlineRun::bounds` and `OutlineRun::EMPTY` are not carried over: both
//! exist for a caller that measures its frame up front, as the launcher's
//! lock-surface memo does. This app gates each run on the window its caller
//! already owns, so neither is called -- and in a bin crate an uncalled public
//! item is a dead-code warning, which is why they are absent rather than
//! allowed. `place` and `pixel`, the two that do the work, are the same code.

use interos_std::design::{font, typeface};

/// Widest run of outline type this module will rasterise, in pixels.
///
/// WHY 640: the widest heading this app draws is the "THERMAL & POWER REGULATOR"
/// card caption, 608px at em 48 over the shipped face's advances, and 640 is the
/// next 64-aligned bound above it -- so every caption the app has is servable
/// rather than refused by the buffer. The app title is well inside it (273px),
/// and 640 keeps the scratch array at a size the guest can afford as BSS.
const RUN_W_MAX: usize = 640;

/// Tallest run, in pixels: ascent plus descent at the largest useful em.
///
/// WHY 64 rather than 96: the largest rung this app serves is em 48, whose line
/// box is 50 + 13 = 63 rows, and 64 is the aligned bound above it. 96 would
/// hold an em of about 72 that no run here uses, at half again the BSS per slot.
const RUN_H_MAX: usize = 64;

/// How many runs may be resident at once.
///
/// WHY two: a surface paints its runs one pixel at a time, so the only way two
/// runs can be live together is one pixel path that tries more than one -- this
/// app's pixel paths try at most one outline run, and a repeat placement of the
/// same run must still find its slot. Two covers both; a third caller has to
/// wait for one of these to be released.
const SLOTS: usize = 2;

/// The longest text a placement key can hold.
///
/// WHY 32: the longest heading is a card caption at 25 bytes ("THERMAL & POWER
/// REGULATOR"), the app title is 14, and `font::fit` may append a three-byte
/// ellipsis to a fitted run -- so 32 covers every run this app can place.
const KEY_MAX: usize = 32;

/// Glyphs whose outline ink leaves the caller's window.
///
/// WHY a list and not a measurement at runtime: the caller's window is the
/// bitmap cell's box (`GLYPH_H * scale` rows from `y0`), and measured against
/// the shipped outline table the ink of these eight glyphs reaches 39.7px
/// above the baseline ('$' at 39.7, '[' and ']' at 39.0, '(' and ')' at 38.5)
/// or 11.0px below it ('(' and ')' at 11.0, '@' at 10.6, 'y' and 'j' at 10.2)
/// at em 48, against a 38px-above / 10px-below budget
/// A run containing one would be cropped by the caller's own window, so it
/// keeps the bitmap face, whose ink is drawn inside its cell by
/// construction. The remaining 87 glyphs fit every window this app draws
/// them in, and no heading this app has contains one of these eight.
const WINDOW_OVERFLOW: &[u8] = b"$()@[]jy";

/// Ink coverage for the placed runs: 255 is fully inked, 0 is untouched.
///
/// WHY `addr_of_mut!` rather than a bare `static mut` reference: the current
/// toolchain warns on taking a reference to a `static mut`, and this array is
/// large enough that copying it is not an option.
static mut COVERAGE: [[u8; RUN_W_MAX * RUN_H_MAX]; SLOTS] =
    [[0; RUN_W_MAX * RUN_H_MAX]; SLOTS];

/// What each slot currently holds, so a repeat placement costs nothing.
static mut LOADED: [Option<Loaded>; SLOTS] = [None, None];

struct Loaded {
    em: u32,
    x: usize,
    y: usize,
    key: [u8; KEY_MAX],
    len: usize,
    w: usize,
    h: usize,
}

/// Which slot a placement lands in.
///
/// WHY this is a search rather than a hash: a hash spreads two runs across two
/// slots only by luck, and two runs that collide silently erase each other.
/// The rule, in order: a slot already holding this exact key is reused, so a
/// repeated placement hits the cache; otherwise an empty slot is taken; and
/// only when every slot holds a different live run is one reused.
fn slot_for(em: u32, x: usize, y: usize, text: &[u8]) -> usize {
    // SAFETY: the read of the cache array, taken and released inside this
    // function. The app's painter is the only caller, it is single threaded,
    // and nothing reached from here re-enters it.
    let loaded = unsafe { &*core::ptr::addr_of!(LOADED) };
    let same = |l: &Loaded| {
        l.em == em && l.x == x && l.y == y && l.len == text.len() && l.key[..text.len()] == *text
    };
    let mut first_free = None;
    for (i, slot) in loaded.iter().enumerate() {
        match slot {
            Some(l) if same(l) => return i,
            Some(_) => {}
            None => {
                if first_free.is_none() {
                    first_free = Some(i);
                }
            }
        }
    }
    first_free.unwrap_or(0)
}

/// One placed run of outline type.
///
/// The run is a plain value: it records where its ink box is and which slot its
/// coverage lives in, so it is as cheap to copy as the bitmap `Run` the app
/// already passes around its frame plans.
#[derive(Clone, Copy)]
pub struct OutlineRun {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    ascent: usize,
    slot: usize,
}

impl OutlineRun {
    /// Places `text` at `em` pixels per em with its baseline on row `y`.
    ///
    /// Returns `None` when the run cannot be rasterised, and the caller is
    /// expected to keep the bitmap face for it. Every refusal here is a case
    /// where drawing would be wrong rather than merely different: a character
    /// the face cannot encode (its advance is zero, so every later glyph would
    /// land on the previous one), a run wider or taller than the scratch
    /// buffer, or a baseline too close to the top of the surface for the
    /// ascender to fit.
    pub fn place(x: usize, y: usize, em: u32, text: &[u8]) -> Option<OutlineRun> {
        if em == 0 || text.is_empty() || text.len() > KEY_MAX {
            return None;
        }
        // An unencodable character advances by zero. Refuse the whole run
        // rather than draw a partly overlapping one.
        if text.iter().any(|&c| typeface::advance_px(c, em) == 0) {
            return None;
        }

        let (ascent, descent) = typeface::line_metrics(em);
        let line_h = (ascent + descent) as usize;
        let w = typeface::text_advance_px(text, em) as usize;
        if w == 0 || w > RUN_W_MAX || line_h > RUN_H_MAX {
            return None;
        }
        // The baseline sits `ascent` rows below the top of the line box, so a
        // run near the top of the surface would be clipped, not drawn.
        if y < ascent as usize {
            return None;
        }

        let mut key = [0u8; KEY_MAX];
        key[..text.len()].copy_from_slice(text);
        let slot = slot_for(em, x, y, text);

        // SAFETY: the mutable access to this slot's cache entry and its buffer,
        // taken and released inside this function. The app's painter is the
        // only caller, it is single threaded, and nothing reached from here
        // re-enters it.
        if let Some(loaded) = unsafe { &*core::ptr::addr_of!(LOADED) }[slot].as_ref() {
            if loaded.em == em
                && loaded.x == x
                && loaded.y == y
                && loaded.len == text.len()
                && loaded.key[..text.len()] == *text
            {
                return Some(OutlineRun {
                    x,
                    y,
                    w: loaded.w,
                    h: loaded.h,
                    ascent: ascent as usize,
                    slot,
                });
            }
        }

        let buf = unsafe { &mut *core::ptr::addr_of_mut!(COVERAGE) };
        let cov = &mut buf[slot][..w * line_h];
        for p in cov.iter_mut() {
            *p = 0;
        }

        // WHY the pen is walked in pixels and not in 16.16: `raster` takes an
        // integer pixel origin and scales internally, and the face's advances
        // are rounded to whole pixels by `advance_px`. A sub-pixel pen would
        // need a fractional origin, which this signature does not offer, and at
        // the sizes this module serves the rounding is below the visible error
        // of a 16x24 bitmap cell -- but it is a known limit, not an oversight.
        let mut pen = 0i32;
        for &c in text {
            let _ = typeface::raster(
                c,
                em,
                pen,
                ascent as i32,
                &mut typeface::Coverage {
                    buf: cov,
                    w,
                    h: line_h,
                },
            );
            pen += typeface::advance_px(c, em) as i32;
        }

        let run = OutlineRun {
            x,
            y,
            w,
            h: line_h,
            ascent: ascent as usize,
            slot,
        };
        // SAFETY: as above -- the mutable access to this slot's cache entry,
        // taken and released here.
        unsafe {
            (*core::ptr::addr_of_mut!(LOADED))[slot] = Some(Loaded {
                em,
                x,
                y,
                key,
                len: text.len(),
                w,
                h: line_h,
            });
        }
        Some(run)
    }

    /// The colour of (`px`, `py`) if this run inks it, blended over `bg`.
    pub fn pixel(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.w == 0 {
            return None;
        }
        let top = self.y.saturating_sub(self.ascent);
        if px < self.x || px - self.x >= self.w || py < top || py - top >= self.h {
            return None;
        }
        let cov = unsafe { &*core::ptr::addr_of!(COVERAGE) };
        let c = cov[self.slot][(py - top) * self.w + (px - self.x)];
        if c == 0 {
            return None;
        }
        Some(font::blend(ink, bg, c))
    }
}

/// Cap height of the bitmap face in cell rows at scale 1.
///
/// WHY defined here rather than in the face: the face's module documents it
/// ("Cap height 17, x-height 12, ascender 16, descender 4") but exports no
/// constant for it, and the derivation that needs it lives here beside the
/// rasteriser it is used against. If the face ever names it, use that instead.
const BITMAP_CAP_ROWS: u64 = 17;

/// Pixels per em that reproduce a bitmap cell's cap height at `scale`.
///
/// WHY this exists: a caller migrating a label from the bitmap face to the
/// outline face must not guess an em, or the text silently changes size. The
/// bitmap face's cap height is `BITMAP_CAP_ROWS` rows at scale 1, so a cell
/// drawn at `scale` has a cap height of `BITMAP_CAP_ROWS * scale` pixels. The
/// outline face's cap height is 1456 of 2048 em -- measured from the shipped
/// `design::face` outline table, where 'H', 'A', 'N' and 'E' all span 1456
/// units -- so solving for em gives this.
///
/// The two faces share a cap height here, but they do not share a width or a
/// total height, and callers must not assume otherwise.
///
/// WHY this is the launcher's function and not a local variant: the derivation
/// is a property of the two faces, not of one app, so every copy of this module
/// in the tree carries the same arithmetic. Two faces of the same cap height
/// must mean the same thing to the shell and to a guest app.
pub fn em_for_scale(scale: usize) -> u32 {
    if scale == 0 {
        return 0;
    }
    // Rounded to nearest so a scale lands on a whole em rather than drifting a
    // fraction of a pixel per glyph, which reads as uneven stroke weight.
    let num = BITMAP_CAP_ROWS * 2048 * scale as u64;
    let den = 1456u64;
    ((num + den / 2) / den) as u32
}

/// Places `text` on the outline face when it can be drawn WHOLE inside the box
/// the caller reserved for it.
///
/// `None` means "keep the bitmap face for this run": a fallback, not a failure,
/// and the caller then draws it exactly as it does today. Because the returned
/// run answers for every pixel of the caller's window, the caller must not also
/// draw the bitmap face for that run in the same frame.
///
/// WHY whole or nothing: the caller's window is the bitmap cell's box, and this
/// app's layout is built on it. A run drawn partly outside that window is
/// cropped by it, which is the silent clip this refuses.
pub fn place_run(
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: Option<usize>,
    bold: bool,
) -> Option<OutlineRun> {
    // Which rungs this serves: only a MAGNIFIED cell. At scale 1 the cell is
    // drawn at its native size, so there is no magnified edge to fix -- the
    // defect this module exists for only appears above 1 -- and it is not
    // expressible either, because at em 24 the face's ink spans 25 rows around
    // the baseline against the cell's 24-row window, so most runs would be
    // refused anyway. The app title and the four card captions are what
    // this app draws above 1, and they are what this module serves; every
    // row, verdict and status line is scale 1 and keeps the bitmap face.
    if scale < 2 {
        return None;
    }
    let em = em_for_scale(scale);
    if bold || text.is_empty() || text.len() > KEY_MAX {
        return None;
    }
    // A glyph the outline cannot encode advances by zero and the whole run is
    // refused by `place`; a glyph whose ink leaves the caller's window is
    // refused here for the same reason -- drawing it would crop it.
    if text.iter().any(|&c| WINDOW_OVERFLOW.contains(&c)) {
        return None;
    }
    // The bitmap face draws its baseline `BASELINE` rows into the cell, so an
    // outline run placed on the same row puts its cap top two rows higher: the
    // face's cap height is measured, not cell-fitted, and that is the one
    // deliberate difference in placement.
    // ascent the line box would start above the surface and `place` refuses it,
    // so the baseline is raised to the ascent instead: the smallest change that
    // keeps every row of the run on the surface. The app title is the run that
    // sits closest to it -- `y0` = 12 against the face's 50-row ascent at
    // em 48 -- and it clears the ascent on its own.
    let (ascent, _) = typeface::line_metrics(em);
    let baseline = (y0 + font::BASELINE * scale).max(ascent as usize);
    let run = OutlineRun::place(x0, baseline, em, text)?;
    // A run wider than the caller's column bound would be cut by the caller's
    // own column test, so it keeps the bitmap face -- which is cut at the same
    // column today.
    if let Some(mw) = max_w {
        if run.w > mw {
            return None;
        }
    }
    Some(run)
}
