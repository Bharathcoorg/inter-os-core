//! Outline type for the gallery's larger text, over the shared rasteriser.
//!
//! WHY this exists alongside the bitmap face: `interos_std::design::font` is a
//! 16x24 cell drawn at an integer multiple, so this app's larger type is that
//! cell magnified. A magnified anti-aliased edge reads as a soft ramp rather
//! than an edge, and no integer multiple of a 24px cell puts a real edge
//! anywhere. `design::typeface` rasterises real outlines at any em, so an edge
//! lands where the outline puts it.
//!
//! THIS IS THE SAME INTEGRATION as `apps/launcher/src/outline_text.rs` and
//! `apps/terminal/src/outline_text.rs` -- `place` / `pixel` over a bounded
//! coverage buffer. Copied rather than shared because these are bin crates with
//! no lib target, so a shared module would have to live in the SDK and every
//! copy here would still need its own bounds.
//!
//! TWO MISTAKES THIS MODULE IS WRITTEN NOT TO REPEAT, both found in the
//! launcher's first version of this change and only caught by looking at the
//! capture:
//!
//! 1. The face is chosen ONCE PER RUN, never per pixel. A per-pixel fallback
//!    draws the glyph from both faces at once -- an outline glyph with a
//!    hard-edged bitmap ghost one pixel off, on every string.
//! 2. The caller's window must cover BOTH renderers. At the em that matches a
//!    scale-2 cap the outline's ascent reaches 18px above the cell origin and
//!    its descent 4px past the bottom, so a cell-sized window clips tall glyphs
//!    silently. `place_run` therefore takes the run's real extents, not the
//!    cell's.

use interos_std::design::{face, typeface};

/// Widest run this app will rasterise, in pixels.
///
/// WHY 320: the gallery's longest label at scale 2 measures 293px against this
/// bound, so the widest thing this app draws fits with room for a longer title.
/// A run past it is refused rather than clipped, and the caller keeps the bitmap
/// face, which never truncates.
const RUN_W_MAX: usize = 320;

/// Tallest run, in pixels: ascent plus descent at the largest em served.
///
/// WHY 96: this app's largest rung is scale 3, whose em is 72 and whose line box
/// is 94 rows. The bound covers it with a row to spare.
const RUN_H_MAX: usize = 96;

/// How many runs may be resident at once.
///
/// WHY two and not one: this app's funnel can be asked for a label and then a
/// caption inside one pixel's resolution, and one slot would have the second
/// erase the first's coverage before it was read.
const SLOTS: usize = 2;

/// The longest text a placement key can hold.
const KEY_MAX: usize = 24;

/// Glyphs whose ink leaves the caller's window at the ems this app serves.
///
/// WHY these and not all of them: measured at em 48 the outline's
/// `$ [ ] ( ) @ j y` reach 39.7px above the baseline, against the 38px budget a
/// scale-2 cell gives. Drawing one would crop it by a row or two, so the whole
/// run is refused and the bitmap face keeps it. The other 87 glyphs in the
/// encoded range fit.
const WINDOW_OVERFLOW: [u8; 11] = [
    b'$', b'[', b']', b'(', b')', b'@', b'j', b'y', b'Q', b'J', b',',
];

static mut COVERAGE: [[u8; RUN_W_MAX * RUN_H_MAX]; SLOTS] =
    [[0; RUN_W_MAX * RUN_H_MAX]; SLOTS];

/// What each slot currently holds, so a repeat placement costs a comparison
/// rather than a re-raster.
///
/// WHY a cache: the funnel is called per pixel, and a run that is placed once
/// per pixel would re-rasterise its whole glyph set for every pixel it inks.
struct Loaded {
    em: u32,
    x0: usize,
    baseline: usize,
    key: [u8; KEY_MAX],
    len: usize,
    w: usize,
    h: usize,
}

static mut LOADED: [Option<Loaded>; SLOTS] = [None, None];

/// Which slot a placement lands in.
///
/// WHY a search rather than a hash: a hash spreads two runs across two slots
/// only by luck, and two that collide silently erase each other -- which is how
/// this class of bug reads at runtime, as letters missing from a label. The
/// order is: reuse the slot already holding this exact key, otherwise take the
/// first empty one, and only when every slot holds a different live run is one
/// reused, at which point the caller has asked for more resident runs than
/// exist and one must give way.
fn slot_for(em: u32, x0: usize, baseline: usize, text: &[u8]) -> usize {
    // SAFETY: the read of the cache array, taken and released inside this
    // function. This app's funnel is single threaded and nothing reached from
    // here re-enters it.
    let loaded = unsafe { &*core::ptr::addr_of!(LOADED) };
    let same = |l: &Loaded| {
        l.em == em
            && l.x0 == x0
            && l.baseline == baseline
            && l.len == text.len()
            && l.key[..text.len()] == *text
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

/// Cap height of the bitmap face in cell rows at scale 1.
///
/// WHY defined here: the face's own module documents it ("Cap height 17,
/// x-height 12, ascender 16, descender 4") but exports no constant for it, and
/// the derivation that needs it lives here beside the rasteriser it is used
/// against.
const BITMAP_CAP_ROWS: u64 = 17;

/// Pixels per em that reproduce a bitmap cell's cap height at `scale`.
///
/// WHY: a caller migrating a label must not guess an em, or the text silently
/// changes size. The bitmap cap height is `BITMAP_CAP_ROWS` rows at scale 1, so
/// a cell at `scale` has a cap height of `BITMAP_CAP_ROWS * scale` pixels; the
/// outline face's cap height is 1456 of 2048 em. Solving for em gives this.
pub fn em_for_scale(scale: usize) -> u32 {
    if scale == 0 {
        return 0;
    }
    // Rounded to nearest so a scale lands on a whole em rather than drifting a
    // fraction of a pixel per glyph, which reads as uneven stroke weight.
    ((BITMAP_CAP_ROWS * 2048 * scale as u64 + 728) / 1456) as u32
}

/// One placed run of outline type.
#[derive(Clone, Copy)]
pub struct OutlineRun {
    x0: usize,
    baseline: usize,
    w: usize,
    h: usize,
    ascent: usize,
    slot: usize,
}

impl OutlineRun {
    /// An empty run, for a funnel that has no outline text.
    ///
    /// WHY `allow(dead_code)`: a bin crate with no lib target warns on an item
    /// nothing in the crate references yet, and this is the documented way for a
    /// caller to say "no outline text here" without inventing a run.
    #[allow(dead_code)]
    pub const EMPTY: OutlineRun = OutlineRun {
        x0: 0,
        baseline: 0,
        w: 0,
        h: 0,
        ascent: 0,
        slot: 0,
    };

    /// Places `text` at the em that matches `scale`, with its baseline
    /// `baseline` rows into the surface.
    ///
    /// `bold` refuses outright: the shipped outline table is Roboto Condensed
    /// Regular, so routing a bold run through it would de-bold the text, which
    /// is a different change from the one this makes.
    ///
    /// Returns `None` for a run that cannot be rasterised, and the caller is
    /// expected to keep the bitmap face for it. Every refusal here is a case
    /// where drawing would be wrong rather than merely different.
    pub fn place_run(
        x0: usize,
        baseline: usize,
        scale: usize,
        text: &[u8],
        bold: bool,
    ) -> Option<OutlineRun> {
        // Only a MAGNIFIED cell has the defect this module exists for. At
        // scale 1 the cell is drawn at its native size, so there is no
        // magnified edge to fix.
        if scale < 2 {
            return None;
        }
        let em = em_for_scale(scale);
        if bold || text.is_empty() || text.len() > KEY_MAX {
            return None;
        }
        // A glyph the face cannot encode advances by zero, so later glyphs would
        // land on earlier ones.
        if text.iter().any(|&c| c < face::FIRST_CHAR || c > face::LAST_CHAR) {
            return None;
        }
        if text.iter().any(|&c| WINDOW_OVERFLOW.contains(&c)) {
            return None;
        }

        let (asc, desc) = typeface::line_metrics(em);
        let line_h = (asc + desc) as usize;
        let w = typeface::text_advance_px(text, em) as usize;
        if w == 0 || w > RUN_W_MAX || line_h > RUN_H_MAX {
            return None;
        }
        // The baseline must sit at least `asc` below the top of the line box, or
        // a tall glyph would be clipped off the surface.
        if baseline < asc as usize {
            return None;
        }

        let mut key = [0u8; KEY_MAX];
        key[..text.len()].copy_from_slice(text);
        let slot = slot_for(em, x0, baseline, text);

        // SAFETY: the read of this slot's cache entry and then the write to its
        // buffer, both taken and released inside this function.
        if let Some(loaded) = unsafe { &*core::ptr::addr_of!(LOADED) }[slot].as_ref() {
            if loaded.em == em
                && loaded.x0 == x0
                && loaded.baseline == baseline
                && loaded.len == text.len()
                && loaded.key[..text.len()] == *text
            {
                return Some(OutlineRun {
                    x0,
                    baseline,
                    w: loaded.w,
                    h: loaded.h,
                    ascent: asc as usize,
                    slot,
                });
            }
        }

        let buf = unsafe { &mut *core::ptr::addr_of_mut!(COVERAGE) };
        let cov = &mut buf[slot][..w * line_h];
        for p in cov.iter_mut() {
            *p = 0;
        }
        let mut pen = 0i32;
        for &c in text {
            let _ = typeface::raster(
                c,
                em,
                pen,
                asc as i32,
                &mut typeface::Coverage {
                    buf: cov,
                    w,
                    h: line_h,
                },
            );
            pen += typeface::advance_px(c, em) as i32;
        }

        let run = OutlineRun {
            x0,
            baseline,
            w,
            h: line_h,
            ascent: asc as usize,
            slot,
        };
        unsafe {
            (*core::ptr::addr_of_mut!(LOADED))[slot] = Some(Loaded {
                em,
                x0,
                baseline,
                key,
                len: text.len(),
                w,
                h: line_h,
            });
        }
        Some(run)
    }

    /// The run's line box as `(x, top, width, height)` in surface rows.
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
        (
            self.x0,
            self.baseline.saturating_sub(self.ascent),
            self.w,
            self.h,
        )
    }

    /// The colour of (`px`, `py`) if this run inks it, blended over `bg`.
    pub fn pixel(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.w == 0 {
            return None;
        }
        let top = self.baseline.saturating_sub(self.ascent);
        if px < self.x0 || px - self.x0 >= self.w || py < top || py - top >= self.h {
            return None;
        }
        let cov = unsafe { &*core::ptr::addr_of!(COVERAGE) };
        let c = cov[self.slot][(py - top) * self.w + (px - self.x0)];
        if c == 0 {
            return None;
        }
        Some(interos_std::design::font::blend(ink, bg, c))
    }
}
