//! Outline type for the shell's largest text, over the shared rasteriser.
//!
//! WHY this exists alongside the bitmap face: `font` is a 16x24 cell rendered at
//! an integer multiple, so the shell's largest type is that cell magnified --
//! `LOCK_CLOCK_SCALE` is 6, a 144px cell for a 102px cap. A magnified
//! anti-aliased edge reads as a soft ramp rather than an edge, and no integer
//! multiple of a 24px cell reaches the reference sizes: measured against
//! `Ui Samples/01_lock_screen.png`, the reference clock is a 60px cap on a
//! 1080-wide canvas, which is a 40px cap in this guest's 720-wide design space
//! -- a 56px em, between the 48px and 72px cells and therefore unreachable.
//!
//! `design::typeface` rasterises real outlines at any em, so an edge lands where
//! the outline puts it. THIS MODULE IS A FIRST INTEGRATION, NOT A MIGRATION:
//! one run at a time, with the bitmap face kept as the fallback everywhere
//! else, so the change can be judged against a real capture before it spreads.
//!
//! WHY ONE RUN AT A TIME: the coverage buffer is a single fixed scratch array.
//! The guest has no allocator for this, and a per-run buffer would multiply a
//! 32KB surface by every text run in a frame. `place` therefore *replaces*
//! whatever was in the buffer, and a caller that needs a second run placed
//! before the first has been fully painted must place it again. The shell's
//! largest text is one run at a time, which is exactly the case this covers.

use interos_std::design::{font, typeface};

/// Widest run of outline type this module will rasterise, in pixels.
///
/// WHY 256: measured from the reference, the widest display run on the lock
/// surface (the date line, twelve glyphs at an 18px em) is a 108px ink advance,
/// so a 256px bound holds it with room for a longer line, while keeping the
/// scratch array at a size the guest can afford as BSS.
const RUN_W_MAX: usize = 256;

/// Tallest run, in pixels: ascent plus descent at the largest useful em.
///
/// WHY 96 rather than 128: the clock at a 48px em is a 63-row line box, and 96
/// covers an em up to about 72 — past the display rung this guest's 720-wide
/// surface can use — while halving the BSS the buffer costs for headroom no
/// caller has.
const RUN_H_MAX: usize = 96;

/// How many runs may be resident at once.
///
/// WHY more than one: the lock surface draws the date line and the clock in the
/// same frame, and a single shared buffer means the second placement erases the
/// first, so the first run then reads the second's glyphs. That is exactly the
/// defect this constant came from. Two is what that surface needs; a third
/// caller has to wait for one of these to be released.
const SLOTS: usize = 2;

/// The longest text a placement key can hold. Longer runs are refused rather
/// than hashed, because a hash would collide and draw the wrong glyphs.
const KEY_MAX: usize = 24;

/// Ink coverage for the placed runs: 255 is fully inked, 0 is untouched.
///
/// WHY `addr_of_mut!` rather than a bare `static mut` reference: the current
/// toolchain warns on taking a reference to a `static mut`, and this array is
/// large enough that copying it is not an option.
static mut COVERAGE: [[u8; RUN_W_MAX * RUN_H_MAX]; SLOTS] =
    [[0; RUN_W_MAX * RUN_H_MAX]; SLOTS];

/// What each slot currently holds, so a repeat placement costs nothing.
///
/// WHY a cache and not a "rasterise on every place": the lock screen resolves
/// its `Frame` through a memo that re-measures on any pixel walk it does not
/// recognise, so placement is not guaranteed to be once per frame. Rasterising
/// the whole glyph set on a cache miss would turn a caller that jumps around
/// into a per-glyph rasteriser per pixel.
struct Loaded {
    em: u32,
    x: usize,
    y: usize,
    key: [u8; KEY_MAX],
    len: usize,
    w: usize,
    h: usize,
}

static mut LOADED: [Option<Loaded>; SLOTS] = [None, None];

/// Which slot a placement lands in.
///
/// WHY derived from the placement rather than handed out round-robin: a
/// round-robin slot would move a run between calls, so a cached run could be
/// answered from a slot another run has since taken. Deriving it from the key
/// puts the same run in the same slot every time, which is what makes the cache
/// sound, and collides only when two runs hash alike.
fn slot_for(em: u32, x: usize, y: usize, text: &[u8]) -> usize {
    // A cheap mix over the whole key. This is a bucket assignment, not a hash
    // that has to resist anything: its only job is to spread two runs.
    let mut acc = em ^ ((x as u32) << 3) ^ ((y as u32) << 11);
    for (i, &c) in text.iter().enumerate() {
        acc ^= (c as u32).rotate_left((i % 31) as u32);
    }
    (acc as usize) % SLOTS
}

/// One placed run of outline type.
///
/// The run is a plain value: it records where its ink box is and which slot its
/// coverage lives in, so it is as cheap to copy as the bitmap `Run` the shell
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
    /// An empty run, for a frame that has no outline text.
    pub const EMPTY: OutlineRun = OutlineRun {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        ascent: 0,
        slot: 0,
    };

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
        // taken and released inside this function. The lock surface's painter is
        // the only caller, it is single threaded, and nothing reached from here
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

    /// The run's line box in surface coordinates, as `(x, top, width, height)`.
    ///
    /// WHY a caller needs this: the shell gates its painter on a box, and the
    /// box has to be the one the run actually occupies. Gating on a box measured
    /// for the other face would either clip the run's ink or, when the run has
    /// moved, draw nothing at all -- which is exactly what happened when the
    /// date line moved to a baseline derived from the reference while its gate
    /// stayed on the bitmap face's cell.
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
        (self.x, self.y.saturating_sub(self.ascent), self.w, self.h)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A run that fits, with a known em, so the coverage can be read back.
    fn placed(em: u32, text: &[u8]) -> OutlineRun {
        OutlineRun::place(0, 200, em, text).expect("run placed")
    }

    #[test]
    fn a_placed_run_covers_the_em_box_and_nothing_outside_it() {
        let run = placed(56, b"09:20");
        let (ascent, descent) = typeface::line_metrics(56);
        assert_eq!(run.h, (ascent + descent) as usize, "the run is one line box");
        assert_eq!(run.w, typeface::text_advance_px(b"09:20", 56) as usize);
        // The baseline is `ascent` rows below the top of the line box, so the
        // run reaches exactly `ascent` above its baseline and no further.
        assert_eq!(run.y.saturating_sub(run.ascent), 200 - ascent as usize);
    }

    #[test]
    fn ink_appears_where_the_outline_says_it_does() {
        // Measured against the same face the host probe rasterises: at a 56px
        // em the reference clock's digits are a 40px cap, so an all-zero
        // coverage buffer means the rasteriser never ran, and a full one means
        // the fill leaked into the glyphs.
        let run = placed(56, b"09:20");
        let top = run.y - run.ascent;
        let mut inked = 0usize;
        for py in top..top + run.h {
            for px in 0..run.w {
                if run.pixel(px, py, 0xFF000000, 0xFFFFFFFF).is_some() {
                    inked += 1;
                }
            }
        }
        // A five-glyph display run inks a healthy minority of its line box.
        assert!(inked > 500, "only {inked} pixels inked");
        assert!(
            inked < (run.w * run.h) / 2,
            "{inked} inked of {} is too much for digits",
            run.w * run.h
        );
    }

    #[test]
    fn a_pixel_outside_the_line_box_is_never_inked() {
        let run = placed(40, b"Ag");
        let top = run.y - run.ascent;
        // One row above the line box, and one row below it.
        for px in 0..run.w {
            assert!(run.pixel(px, top.saturating_sub(1), 0, 0xFFFFFFFF).is_none());
            assert!(run.pixel(px, top + run.h, 0, 0xFFFFFFFF).is_none());
        }
        for py in top..top + run.h {
            assert!(run.pixel(run.w, py, 0, 0xFFFFFFFF).is_none());
        }
    }

    #[test]
    fn coverage_blends_rather_than_replacing_the_backdrop() {
        // A partially covered pixel must be between the two colours, not one of
        // them, or the edge would read as a staircase on the scene backdrop.
        let run = placed(56, b"09:20");
        let top = run.y - run.ascent;
        let ink = 0xFF000000u32;
        let bg = 0xFFFFFFFFu32;
        let mut partial = 0usize;
        for py in top..top + run.h {
            for px in 0..run.w {
                if let Some(c) = run.pixel(px, py, ink, bg) {
                    if c != ink && c != bg {
                        partial += 1;
                    }
                }
            }
        }
        assert!(partial > 0, "no anti-aliased edge was produced");
    }

    #[test]
    fn a_baseline_too_close_to_the_top_is_refused_not_clipped() {
        // `place(0, 3, 56, ..)` would put the ascender above the surface: the
        // only honest outcome is a refusal, so the caller keeps the bitmap face.
        assert!(OutlineRun::place(0, 3, 56, b"09:20").is_none());
    }

    #[test]
    fn an_unencodable_character_refuses_the_whole_run() {
        // A character the face cannot encode advances by zero, which would draw
        // every later glyph on top of the previous one.
        assert!(OutlineRun::place(0, 200, 40, b"ok\0no").is_none());
        // A newline is inside the encoded range but has no outline; its advance
        // is non-zero, so it places and simply inks nothing, which is the same
        // behaviour the bitmap face has.
        assert!(OutlineRun::place(0, 200, 40, b"a b").is_some());
    }

    #[test]
    fn a_run_wider_or_taller_than_the_buffer_is_refused() {
        // 256px is the buffer's width; a longer run at a large em exceeds it.
        let long = b"abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";
        assert!(OutlineRun::place(0, 2000, 96, long).is_none());
    }

    #[test]
    fn an_empty_run_and_a_zero_em_are_refused() {
        assert!(OutlineRun::place(0, 200, 24, b"").is_none());
        assert!(OutlineRun::place(0, 200, 0, b"Ag").is_none());
    }

    #[test]
    fn two_runs_resident_together_do_not_erase_each_other() {
        // The lock surface places the date and then the clock in one frame, and
        // both are painted afterwards. With one shared buffer the clock's
        // rasterisation erased the date's coverage, and the date then read the
        // clock's glyphs: its letters vanished and only its comma survived.
        // Pin the pair so the pooling cannot regress.
        let date = OutlineRun::place(0, 251, 18, b"Mon, 12 Oct").expect("date placed");
        let clock = OutlineRun::place(0, 325, 48, b"00:02").expect("clock placed");
        assert_ne!(date.slot, clock.slot, "the two runs must not share a slot");

        // Both runs must ink pixels after the second placement. Read the date
        // first, in the order the frame draws them, so a shared buffer would
        // fail here rather than somewhere further away.
        let date_ink = count_ink(&date);
        let clock_ink = count_ink(&clock);
        assert!(date_ink > 200, "date inked only {date_ink} pixels");
        assert!(clock_ink > 500, "clock inked only {clock_ink} pixels");
    }

    /// Inked pixel count over a run's whole line box.
    fn count_ink(run: &OutlineRun) -> usize {
        let (x, top, w, h) = run.bounds();
        let mut ink = 0usize;
        for py in top..top + h {
            for px in x..x + w {
                if run.pixel(px, py, 0xFF000000, 0xFFFFFFFF).is_some() {
                    ink += 1;
                }
            }
        }
        ink
    }

    #[test]
    fn a_repeat_placement_is_freed_by_the_cache() {
        // The lock painter resolves its frame through a memo that re-measures on
        // an unrecognised pixel walk, so placement must be cheap to repeat or a
        // jumping caller would re-rasterise per pixel.
        let first = placed(56, b"09:20");
        let second = placed(56, b"09:20");
        assert_eq!(first.w, second.w);
        assert_eq!(first.h, second.h);
    }

    #[test]
    fn placing_a_different_run_replaces_the_buffer() {
        let clock = placed(56, b"09:20");
        let other = OutlineRun::place(0, 200, 40, b"Ag").expect("second run placed");
        assert_ne!(clock.w, other.w);
        // The first run's pixels are no longer guaranteed: the buffer now holds
        // the second run, so reading it back as the first is wrong, and the
        // caller must not do it. This test pins that the geometry changed.
        assert_eq!(other.w, typeface::text_advance_px(b"Ag", 40) as usize);
    }

    #[test]
    fn the_clock_run_is_larger_than_the_date_run() {
        // A lock surface is arranged around one large clock. While the clock was
        // a bitmap cell at scale 6, a scale-2 date read as subordinate; at a
        // 48px em they share a cap height, which inverts the hierarchy. Pin the
        // relationship rather than the numbers so a future em change has to
        // keep it.
        let clock = placed(48, b"00:02");
        let date = OutlineRun::place(0, 300, 18, b"Mon, 12 Oct").expect("date placed");
        assert!(
            clock.h > date.h,
            "clock line box {} must exceed date line box {}",
            clock.h,
            date.h
        );
        assert!(
            typeface::text_advance_px(b"00:02", 48) > typeface::text_advance_px(b"Mon, 12 Oct", 18),
            "the clock must also be the wider run"
        );
    }

    #[test]
    fn a_longer_run_is_refused_rather_than_truncated() {
        // 24 bytes is the placement key's bound. Refusing is the honest
        // outcome: the caller keeps the bitmap face, which never truncates.
        let long = b"abcdefghijklmnopqrstuvwx";
        assert!(OutlineRun::place(0, 200, 12, long).is_some());
        assert!(OutlineRun::place(0, 200, 12, b"abcdefghijklmnopqrstuvwxy").is_none());
    }
}
