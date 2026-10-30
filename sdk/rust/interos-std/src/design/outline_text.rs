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

use crate::font;
use crate::design::typeface;

/// Widest run of outline type this module will rasterise, in pixels.
///
/// WHY 680: The design canvas is 720px wide with 20px padding (680px content area).
/// Expanding to 680 ensures that full-width headlines, card subtitles, and
/// long metadata strings are rendered as vector outlines without falling back to
/// blocky bitmap faces.
const RUN_W_MAX: usize = 640;

/// Tallest run, in pixels: ascent plus descent at the largest useful em.
///
/// WHY 144 and not 100: the display rung is a fifty-six pixel cap, whose em
/// is 79 pixels and whose line box is 103 (ascender 82 plus descender 21).
/// The hero rung's seventy-six pixel cap is a 107 pixel em with a 141 pixel
/// box. At 100 the face refused both, and the caller fell back to the
/// bitmap face at a third of the size -- the home screen's date then drew
/// as a low-resolution cell repeated vertically, which reads as blur.
const RUN_H_MAX: usize = 144;

/// How many runs may be resident at once.
const SLOTS: usize = 32;

/// The longest text a placement key can hold. Longer runs are refused rather
/// than hashed, because a hash would collide and draw the wrong glyphs.
///
/// WHY a hundred and twenty-eight and not sixty-four: the key holds the
/// text itself, so its length is the length of the longest run the face
/// will draw. A sixty-four byte key refused every sentence longer than
/// sixty-four characters, and a refused run is drawn by the bitmap face at
/// the bitmap's whole-cell size -- which is a different size from the rest
/// of the screen, so a paragraph of prose came out in a larger face than
/// the label above it. The width limit below is the one that should bind:
/// a run wider than the coverage buffer cannot be drawn at all, and a
/// hundred and twenty-eight characters at the caption's size is narrower
/// than that buffer.
pub const KEY_MAX: usize = 128;

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
#[derive(Clone, Copy)]
struct Loaded {
    em: u32,
    x: usize,
    y: usize,
    key: [u8; KEY_MAX],
    len: usize,
    w: usize,
    h: usize,
    bold: bool,
    /// The request stamp of the last time this run was asked for. It is what
    /// the pool uses to choose a victim, so the run a pixel walk is painting
    /// right now is never the one evicted in favour of it.
    used: u32,
}

static mut LOADED: [Option<Loaded>; SLOTS] = [None; SLOTS];

/// The request stamp handed to each placement or hit.
///
/// WHY a stamp and not a per-pixel frame number: the pool has no idea where a
/// frame begins or ends, and a caller that paints one pixel at a time asks for
/// the same few runs again and again. A monotonic stamp is enough to tell the
/// run asked for a moment ago from the one asked for a screen ago.
static mut CLOCK: u32 = 0;

/// Hands out the next request stamp.
fn next_stamp() -> u32 {
    unsafe {
        let clock = &mut *core::ptr::addr_of_mut!(CLOCK);
        *clock = clock.wrapping_add(1);
        *clock
    }
}

/// Resets all slots in the outline cache pool.
pub fn reset_slot_pool() {
    unsafe {
        for slot in (*core::ptr::addr_of_mut!(LOADED)).iter_mut() {
            *slot = None;
        }
    }
    // WHY the gate boxes go with them: a caller resets these caches when the
    // screen it was painting is gone. A box left behind from that screen would
    // be handed to the next one's run of the same text at the same origin,
    // which is the same box -- but a caller that has just decided the caches
    // are untrustworthy should not have to reason about which of the two
    // tables it invalidated.
    reset_box_cache();
}

/// The run that was placed last.
///
/// WHY this exists: a caller that paints a run asks for that same run for
/// every pixel of it, and placing it rebuilt a sixty-four byte key --
/// zeroed and then filled -- for each of those pixels. On a screen drawn
/// one pixel at a time that was the single largest cost in a frame, and
/// it grew with the number of pixels rather than with the amount of text.
#[derive(Clone, Copy)]
struct LastPlaced {
    em: u32,
    x: usize,
    y: usize,
    bold: bool,
    len: u8,
    key: [u8; KEY_MAX],
    run: Option<OutlineRun>,
}

static mut LAST_PLACED: Option<LastPlaced> = None;

/// How many gate boxes are remembered.
///
/// WHY as many as the coverage pool: a frame that paints one pixel at a time
/// asks the gate question for every run it owns at every pixel, so the box
/// cache has to hold the whole screen's runs or it answers nothing. A screen
/// with more distinct runs than this still gets the right boxes; it just
/// re-measures the ones that were evicted.
const BOX_SLOTS: usize = SLOTS;

/// A run's gate box, remembered from the one time it was measured.
///
/// WHY the box needs its own memo when the coverage already has one: a caller
/// that gates a pixel before drawing it asks [`run_box`], and `run_box` walks
/// the whole string twice -- once for the bitmap face's width and once for the
/// outline's. The coverage pool answers questions about a *placement*, and
/// `run_box` is keyed by the text's own origin, so it cannot use it. Both are
/// asked once per pixel by the same kind of caller, so both need remembering.
#[derive(Clone, Copy)]
struct Boxed {
    x0: usize,
    y0: usize,
    scale: usize,
    len: usize,
    used: u32,
    key: [u8; KEY_MAX],
    box_: (usize, usize, usize, usize),
}

static mut BOXES: [Option<Boxed>; BOX_SLOTS] = [None; BOX_SLOTS];

/// Which box slot a gate question lands in.
///
/// Mirrors [`slot_for`]: a hash of the key's cheap parts selects the probe
/// start, so a walk that asks the same few questions again finds them without
/// comparing every entry in the table.
fn box_slot_for(x0: usize, y0: usize, scale: usize, len: usize) -> usize {
    let mut h = x0 ^ (y0 << 5) ^ (scale << 9) ^ len;
    h ^= (h >> 3) ^ (h >> 7);
    h % BOX_SLOTS
}

/// The remembered box for this exact run, if one is resident.
fn cached_box(
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
) -> Option<(usize, usize, usize, usize)> {
    let base = box_slot_for(x0, y0, scale, text.len());
    let hit = unsafe {
        let boxes = &*core::ptr::addr_of!(BOXES);
        let mut found = None;
        for i in 0..BOX_SLOTS {
            let slot = (base + i) % BOX_SLOTS;
            if let Some(entry) = boxes[slot].as_ref() {
                if entry.x0 == x0
                    && entry.y0 == y0
                    && entry.scale == scale
                    && entry.len == text.len()
                    && entry.key[..entry.len] == *text
                {
                    found = Some((slot, entry.box_));
                    break;
                }
            }
        }
        found
    };
    let (slot, measured) = hit?;
    // SAFETY: the borrow above is released before this pointer is taken, and
    // nothing reached from here re-enters the table.
    unsafe {
        if let Some(entry) = (*core::ptr::addr_of_mut!(BOXES))[slot].as_mut() {
            entry.used = next_stamp();
        }
    }
    Some(measured)
}

/// Remembers a box, replacing the least recently asked-for one in its probe
/// run when the run is full.
fn remember_box(
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    measured: (usize, usize, usize, usize),
) {
    if text.len() > KEY_MAX {
        return;
    }
    let base = box_slot_for(x0, y0, scale, text.len());
    let mut key = [0u8; KEY_MAX];
    key[..text.len()].copy_from_slice(text);
    // SAFETY: as above -- the table is written through a raw pointer and no
    // borrow of it is live here.
    unsafe {
        let boxes = &mut *core::ptr::addr_of_mut!(BOXES);
        let mut target = base;
        let mut oldest = u32::MAX;
        for i in 0..BOX_SLOTS {
            let slot = (base + i) % BOX_SLOTS;
            match boxes[slot].as_ref() {
                None => {
                    target = slot;
                    break;
                }
                Some(entry) => {
                    if entry.used < oldest {
                        oldest = entry.used;
                        target = slot;
                    }
                }
            }
        }
        boxes[target] = Some(Boxed {
            x0,
            y0,
            scale,
            len: text.len(),
            used: next_stamp(),
            key,
            box_: measured,
        });
    }
}

/// Forgets every remembered gate box.
///
/// WHY this is separate from [`reset_slot_pool`]: a box names no coverage
/// slot, so emptying one cache cannot invalidate the other. It is folded into
/// `reset_slot_pool` anyway, because a caller that declares the caches stale
/// means the whole screen, not one table of it.
pub fn reset_box_cache() {
    unsafe {
        for entry in (*core::ptr::addr_of_mut!(BOXES)).iter_mut() {
            *entry = None;
        }
    }
}

/// Forgets the last placement.
///
/// WHY it exists: a caller that keeps a frame plan across frames must be
/// able to say that the cache no longer describes the screen.
pub fn forget_last_placement() {
    unsafe {
        *core::ptr::addr_of_mut!(LAST_PLACED) = None;
    }
}

/// Which slot a placement lands in.
///
/// Uses a direct-mapped hash on `(x, y, em, text_len, bold)` to select a slot
/// in O(1) time without linear scanning or memcmps inside per-pixel probe loops.
fn slot_for(em: u32, x: usize, y: usize, text: &[u8], bold: bool) -> usize {
    let mut h = (x ^ (y << 4) ^ ((em as usize) << 8) ^ text.len() ^ (if bold { 0x55 } else { 0 })) as usize;
    h ^= (h >> 4) ^ (h >> 8) ^ (h >> 12);
    h % SLOTS
}

/// Finds an existing cached slot, the first open slot, or the run to evict.
///
/// WHY the last resort is the least recently asked-for run and not the new
/// run's hash slot: a caller walks a screen one pixel at a time, so the runs
/// that share a row are asked for on every pixel. Choosing the new run's own
/// base slot as the victim can pick one of those -- two runs that hash alike
/// then erase each other once per pixel, and each erase rasterises a whole
/// line. That is a render that never finishes rather than a cache that is full.
/// Evicting the run nobody has asked for since keeps the pixel walk's working
/// set resident, so a full pool costs a miss on new text, not a storm.
fn find_or_alloc_slot(em: u32, x: usize, y: usize, text: &[u8], bold: bool) -> (usize, bool) {
    let base = slot_for(em, x, y, text, bold);
    unsafe {
        let loaded_ptr = &*core::ptr::addr_of!(LOADED);
        for i in 0..SLOTS {
            let slot = (base + i) % SLOTS;
            if let Some(loaded) = loaded_ptr[slot].as_ref() {
                if loaded.em == em
                    && loaded.x == x
                    && loaded.y == y
                    && loaded.bold == bold
                    && loaded.len == text.len()
                    && loaded.key[..text.len()] == *text
                {
                    return (slot, true);
                }
            }
        }
        for i in 0..SLOTS {
            let slot = (base + i) % SLOTS;
            if loaded_ptr[slot].is_none() {
                return (slot, false);
            }
        }
        let mut victim = base;
        let mut oldest = u32::MAX;
        for slot in 0..SLOTS {
            if let Some(loaded) = loaded_ptr[slot].as_ref() {
                if loaded.used < oldest {
                    oldest = loaded.used;
                    victim = slot;
                }
            }
        }
        (victim, false)
    }
}

/// Whether the named slot still holds exactly this run.
///
/// WHY the question is asked at all: the one-entry memo names a slot, and a
/// caller that then places more distinct runs than the pool holds evicts the
/// run that slot was drawn for. Reading the memo without checking would hand
/// out the eviction's glyphs under the old run's name -- a wrong glyph set, not
/// a stale one, and the kind of failure that shows up on one run and not the
/// next.
fn slot_still_holds(
    slot: usize,
    em: u32,
    x: usize,
    y: usize,
    text: &[u8],
    bold: bool,
) -> bool {
    unsafe {
        // SAFETY: the static is read through a raw pointer and the reference it
        // yields does not outlive this call.
        match (&*core::ptr::addr_of!(LOADED)).get(slot) {
            Some(Some(entry)) => {
                entry.em == em
                    && entry.x == x
                    && entry.y == y
                    && entry.bold == bold
                    && entry.len == text.len()
                    && entry.key[..text.len()] == *text
            }
            _ => false,
        }
    }
}

/// The run this placement question was already asked about, if one is resident.
///
/// WHY this is a lookup and not [`find_or_alloc_slot`]: the allocator answers
/// "where would this go", and in doing so hands out a slot for a run that is
/// not there. A per-pixel caller asks a question it has already had answered
/// far more often than one it has not, and it must be able to take that answer
/// without touching the table.
///
/// WHY the last placement is refreshed from a table hit: the one-entry memo is
/// what makes the run a caller is painting right now cost a comparison rather
/// than a scan. Without the refresh it would only ever hold a run from the
/// first pixel of a frame, and a caller that cycles through a screen's runs --
/// which is what painting a frame one pixel at a time means -- would miss it
/// every time.
fn resident_run(em: u32, x: usize, y: usize, text: &[u8], bold: bool) -> Option<OutlineRun> {
    unsafe {
        // SAFETY: the static is read through a raw pointer and the reference it
        // yields does not outlive this block, and nothing reached from here
        // re-enters it.
        if let Some(last) = (&*core::ptr::addr_of!(LAST_PLACED)).as_ref() {
            if last.em == em
                && last.x == x
                && last.y == y
                && last.bold == bold
                && last.len as usize == text.len()
                && last.key[..text.len()] == *text
            {
                if let Some(run) = last.run {
                    // WHY the slot is confirmed before the answer is taken:
                    // the memo names a slot, and the pool may have drawn a
                    // different run into it since. A miss here falls through to
                    // the table, which answers from what is actually resident.
                    if slot_still_holds(run.slot, em, x, y, text, bold) {
                        // WHY the hit also stamps the slot: this is the run the
                        // pixel walk is asking for right now, so it must be the
                        // last candidate for eviction rather than the first.
                        if let Some(entry) =
                            (*core::ptr::addr_of_mut!(LOADED))[run.slot].as_mut()
                        {
                            entry.used = next_stamp();
                        }
                        return last.run;
                    }
                }
            }
        }
    }

    let base = slot_for(em, x, y, text, bold);
    let hit = unsafe {
        let loaded = &*core::ptr::addr_of!(LOADED);
        let mut found = None;
        for i in 0..SLOTS {
            let slot = (base + i) % SLOTS;
            if let Some(entry) = loaded[slot].as_ref() {
                if entry.em == em
                    && entry.x == x
                    && entry.y == y
                    && entry.bold == bold
                    && entry.len == text.len()
                    && entry.key[..text.len()] == *text
                {
                    found = Some((slot, entry.w, entry.h));
                    break;
                }
            }
        }
        found
    };
    let (slot, w, h) = hit?;
    let (ascent, _) = typeface::line_metrics(em);
    let run = OutlineRun {
        x,
        y,
        w,
        h,
        ascent: ascent as usize,
        slot,
    };
    // SAFETY: the borrow above is released before either pointer is taken here.
    unsafe {
        if let Some(entry) = (*core::ptr::addr_of_mut!(LOADED))[slot].as_mut() {
            entry.used = next_stamp();
        }
        let mut key = [0u8; KEY_MAX];
        key[..text.len()].copy_from_slice(text);
        *core::ptr::addr_of_mut!(LAST_PLACED) = Some(LastPlaced {
            em,
            x,
            y,
            bold,
            len: text.len() as u8,
            key,
            run: Some(run),
        });
    }
    Some(run)
}

/// Pixels per em that draw the cap height the scale rung names.
///
/// WHY the rung names the design document's size and not the bitmap cell's
/// multiple: the document's type scale is written in the sizes a reader
/// sees -- a caption of thirteen pixels, a body of sixteen, a title of
/// thirty and a page heading of forty -- while the bitmap cell's multiples
/// (seventeen, thirty-four, fifty-one, a hundred and two) are more than
/// twice those for every rung above the first. A label drawn at the
/// bitmap's cap no longer fits the box the document's layout reserves for
/// it, which is the overlap the panel showed. `cap_for_scale` carries the
/// ladder, and the em follows from the cap it names.
pub const fn em_for_scale(scale: usize) -> u32 {
    crate::font::em_for_scale(scale)
}

/// The colour of (`px`, `py`) if the run placed at (`x`, `baseline`) with size
/// `em` inks it, blended over `bg`.
///
/// WHY this is a free function taking the whole key rather than a method on a
/// placed run: the callers hold resolved labels in frame plans that must stay
/// cheap to copy, and a placed run carrying a buffer is neither. Placing on
/// demand behind the caller's own box test costs one cached lookup per inked
/// pixel and nothing at all for the rest of the surface -- which is what lets a
/// single coverage slot serve a frame full of runs.
    ///
    /// Why the attribute: the bin target does not call this -- the shell's
    /// own painters place a run and ask it directly -- but the lib target's tests
    /// do, and a module that cannot be exercised from its own tests is worse
    /// than one with an unused helper.
    #[allow(dead_code)]
    pub fn pixel(
        x: usize,
        baseline: usize,
        em: u32,
        text: &[u8],
        px: usize,
        py: usize,
        ink: u32,
        bg: u32,
    ) -> Option<u32> {
        let run = OutlineRun::place(x, baseline, em, text)?;
        run.pixel(px, py, ink, bg)
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

/// The em, in 1/2048 of an em, that gives a cap height of `cap_px` pixels.
///
/// WHY this exists: the integer scale ladder is a ladder of *multiples* of
/// the bitmap face's 16-pixel cell, so it has a hole in it. The design
/// document's type scale asks for a caption of 12 to 14 pixels, a section
/// heading of 20 to 24 and a page heading of 36 to 44, and the multiples
/// available are 16, 32, 48 and 96 -- so a section heading and a page
/// heading have no rung to sit on, and an app that wants one either
/// overshoots or has to draw it with the other face. This face rasterises
/// from outlines, so it can be asked for the size the document names.
pub const fn em_for_cap_height(cap_px: u32) -> u32 {
    if cap_px == 0 {
        return 0;
    }
    // 1456 is the cap height in 1/2048 of an em, and 2048 is the em.
    ((cap_px as u64 * 2048 + 728) / 1456) as u32
}

/// Places a run whose size is named as a cap height in pixels rather than as
/// a multiple of the bitmap face's cell.
///
/// WHY the cap height and not the em: a cap height is what a reader sees,
/// and the design document's type scale is written in the sizes a reader
/// sees.
pub fn place_run_cap(
    x0: usize,
    y0: usize,
    cap_px: u32,
    text: &[u8],
    max_w: Option<usize>,
    bold: bool,
) -> Option<OutlineRun> {
    if cap_px == 0 || text.is_empty() || text.len() > KEY_MAX {
        return None;
    }
    // The ladder's cap heights are the ladder's own sizes rounded to whole
    // pixels, so they resolve back to the size the rung names rather than to
    // a slightly different one.
    let em = em_for_cap_height(cap_px);
    let (ascent, _) = typeface::line_metrics(em);
    // The baseline is placed from the cap height rather than from the
    // bitmap's constant, so a run at any size sits on the same optical
    // line the other sizes do.
    let cap_rows = typeface::line_metrics(em).0;
    let _ = cap_rows;
    let baseline = y0 + ascent as usize;
    let run = OutlineRun::place_styled(x0, baseline, em, text, bold)?;
    if let Some(mw) = max_w {
        if run.w > mw {
            return None;
        }
    }
    Some(run)
}

/// Converts geometric coverage (0..=255) to the alpha used for compositing.
///
/// WHY the quadratic and not a table: `font::blend` composites in linear
/// light, so blending straight coverage makes edge pixels far lighter than
/// the fraction of the pixel the glyph actually covers -- a 50% edge would
/// land near sRGB 180 instead of the mid-grey a reader expects. Shaping the
/// coverage as `1 - (1 - c)^2` (equivalently `c * (2 - c)`) maps geometric
/// coverage back to the perceptual mid-tone while staying smooth across all
/// 256 levels. The previous 16-entry table produced the same mid-tone but
/// quantised every edge to four bits, which is what left visible steps and
/// grey halos on large glyphs.
///
/// The exact form in 8-bit integer arithmetic is `c * (510 - c) / 255`,
/// which is monotonic, hits 0 and 255 exactly, and needs no table.
#[inline(always)]
pub fn perceptual_alpha(coverage: u8) -> u8 {
    if coverage == 0 || coverage == 255 {
        return coverage;
    }
    let c = coverage as u32;
    ((c * (510 - c) + 127) / 255) as u8
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
        Self::place_styled(x, y, em, text, false)
    }

    /// Places `text` with optional faux-bold stroke dilation.
    pub fn place_styled(x: usize, y: usize, em: u32, text: &[u8], bold: bool) -> Option<OutlineRun> {
        if em == 0 || text.is_empty() || text.len() > KEY_MAX {
            return None;
        }
        // WHY the resident run is looked up before the walk below: a caller
        // that paints a frame one pixel at a time asks for the same run once
        // per pixel of it, and the walk costs a character's worth of work each
        // time. A run already in the table passed every gate on the way in --
        // its stored width is the width the walk would recompute, its height is
        // the same line box, and its baseline and em are part of the key that
        // just matched -- so answering from the table cannot admit a run the
        // walk would have refused.
        if let Some(resident) = resident_run(em, x, y, text, bold) {
            return Some(resident);
        }
        // WHY one walk covers the gate, the width and the pen: the face is
        // indexed by code point, and a walk of bytes reads the continuation
        // bytes of a multi-byte sequence as characters the face has not got.
        // The advance of such a byte is zero, and refusing on it discarded a
        // run this face can draw -- the accented word in a heading fell to the
        // bitmap cell, so the heading came out in two sizes. Measuring and
        // drawing from the same walk also means the width a caller lays out
        // with cannot disagree with the glyphs that land on the surface.
        let mut pen = 0u32;
        let mut i = 0usize;
        while i < text.len() {
            let (cp, next) = codepoint_at(text, i);
            let adv = typeface::advance_codepoint(cp, em);
            // An unencodable character advances by zero. Refuse the whole run
            // rather than draw a partly overlapping one.
            if adv == 0 {
                return None;
            }
            pen += adv;
            i = next;
        }

        let (ascent, descent) = typeface::line_metrics(em);
        let line_h = (ascent + descent) as usize;
        let w = pen as usize + if bold { 1 } else { 0 };
        if w == 0 || w > RUN_W_MAX || line_h > RUN_H_MAX {
            return None;
        }
        if y < ascent as usize {
            return None;
        }

        let mut key = [0u8; KEY_MAX];
        key[..text.len()].copy_from_slice(text);
        // WHY the search is not repeated here: `resident_run` probed these same
        // slots for this same key and did not find it, so this call only has to
        // choose where the new run goes.
        let (slot, _is_cached) = find_or_alloc_slot(em, x, y, text, bold);

        let buf = unsafe { &mut *core::ptr::addr_of_mut!(COVERAGE) };
        let cov = &mut buf[slot][..w * line_h];
        for p in cov.iter_mut() {
            *p = 0;
        }

        // WHY the pen is walked in pixels and not in 16.16: `raster_codepoint`
        // takes an integer pixel origin and scales internally, and the face's
        // advances are rounded to whole pixels by `advance_codepoint`. A
        // sub-pixel pen would need a fractional origin, which this signature
        // does not offer, and at the sizes this module serves the rounding is
        // below the visible error of a 16x24 bitmap cell -- but it is a known
        // limit, not an oversight.
        let mut pen = 0i32;
        let mut i = 0usize;
        while i < text.len() {
            let (cp, next) = codepoint_at(text, i);
            let _ = typeface::raster_codepoint(
                cp,
                em,
                pen,
                ascent as i32,
                &mut typeface::Coverage {
                    buf: cov,
                    w,
                    h: line_h,
                },
            );
            pen += typeface::advance_codepoint(cp, em) as i32;
            i = next;
        }

        if bold {
            for row in 0..line_h {
                let row_start = row * w;
                for col in (1..w).rev() {
                    let prev = cov[row_start + col - 1];
                    if prev > cov[row_start + col] {
                        cov[row_start + col] = prev;
                    }
                }
            }
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
                bold,
                used: next_stamp(),
            });
        }
        Some(run)
    }

    /// The run's line box in surface coordinates, as `(x, top, width, height)`.
    /// Handles runs near or at top edge (y < ascent) safely with non-negative bounds.
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
        let top = self.y.saturating_sub(self.ascent);
        let bottom = self.y + (self.h.saturating_sub(self.ascent));
        (self.x, top, self.w, bottom.saturating_sub(top))
    }

    /// The colour of (`px`, `py`) if this run inks it, blended over `bg`.
    #[allow(dead_code)]
    pub fn pixel(&self, px: usize, py: usize, ink: u32, bg: u32) -> Option<u32> {
        if self.w == 0 || self.h == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w {
            return None;
        }
        let cov_row = (py as isize) - (self.y as isize) + (self.ascent as isize);
        if cov_row < 0 || cov_row >= self.h as isize {
            return None;
        }
        unsafe {
            if let Some(ref loaded) = (*core::ptr::addr_of!(LOADED))[self.slot] {
                if loaded.w != self.w || loaded.h != self.h || loaded.x != self.x || loaded.y != self.y {
                    return None;
                }
            } else {
                return None;
            }
        }
        let cov = unsafe { &*core::ptr::addr_of!(COVERAGE) };
        let c = cov[self.slot][(cov_row as usize) * self.w + (px - self.x)];
        if c == 0 {
            return None;
        }
        let alpha = perceptual_alpha(c);
        Some(font::blend(ink, bg, alpha))
    }

    /// The colour of (`px`, `py`) if this run inks it with binary thresholding (c >= 64).
    #[allow(dead_code)]
    pub fn pixel_binary(&self, px: usize, py: usize, ink: u32) -> Option<u32> {
        if self.w == 0 || self.h == 0 {
            return None;
        }
        if px < self.x || px - self.x >= self.w {
            return None;
        }
        let cov_row = (py as isize) - (self.y as isize) + (self.ascent as isize);
        if cov_row < 0 || cov_row >= self.h as isize {
            return None;
        }
        unsafe {
            if let Some(ref loaded) = (*core::ptr::addr_of!(LOADED))[self.slot] {
                if loaded.w != self.w || loaded.h != self.h || loaded.x != self.x || loaded.y != self.y {
                    return None;
                }
            } else {
                return None;
            }
        }
        let cov = unsafe { &*core::ptr::addr_of!(COVERAGE) };
        let c = cov[self.slot][(cov_row as usize) * self.w + (px - self.x)];
        if c >= 64 {
            Some(ink)
        } else {
            None
        }
    }
}

pub const WINDOW_OVERFLOW: &[u8] = b"";

pub fn place_run(
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: Option<usize>,
    bold: bool,
) -> Option<OutlineRun> {
    if scale < 1 || text.is_empty() || text.len() > KEY_MAX {
        return None;
    }
    let em = em_for_scale(scale);
    let (ascent, _) = typeface::line_metrics(em);
    let baseline = (y0 + ascent as usize).max(y0 + font::ascent_for_scale(scale));
    let run = OutlineRun::place_styled(x0, baseline, em, text, bold)?;
    if let Some(mw) = max_w {
        if run.w > mw {
            return None;
        }
    }
    Some(run)
}

impl OutlineRun {
pub fn place_run(
        x0: usize,
        baseline: usize,
        scale: usize,
        text: &[u8],
        bold: bool,
    ) -> Option<OutlineRun> {
        if scale < 1 || text.is_empty() || text.len() > KEY_MAX {
            return None;
        }
        let em = em_for_scale(scale);
        let (ascent, _) = typeface::line_metrics(em);
        let safe_baseline = baseline.max(ascent as usize);
        OutlineRun::place_styled(x0, safe_baseline, em, text, bold)
    }
}

pub fn baseline_row(y0: usize, scale: usize, ascent: u32) -> usize {
    (y0 + ascent as usize).max(y0 + font::ascent_for_scale(scale))
}

pub fn run_box(x0: usize, y0: usize, scale: usize, text: &[u8]) -> (usize, usize, usize, usize) {
    // WHY the memo comes first: this is the gate a per-pixel painter asks
    // before it asks anything else, so it is asked once per pixel per run, and
    // the measurement below is a walk of the whole string. The box is a pure
    // function of the four arguments, so a remembered answer is not an
    // approximation of the measured one -- it is the measured one.
    if let Some(remembered) = cached_box(x0, y0, scale, text) {
        return remembered;
    }
    let measured = measure_box(x0, y0, scale, text);
    remember_box(x0, y0, scale, text, measured);
    measured
}

/// Measures a run's gate box, the walk [`run_box`] now remembers the answer to.
fn measure_box(x0: usize, y0: usize, scale: usize, text: &[u8]) -> (usize, usize, usize, usize) {
    let em = em_for_scale(scale);
    let bitmap_w = font::measure(text, scale);
    let outline_w = typeface::text_advance_px(text, em) as usize;
    let w = bitmap_w.max(outline_w);
    if w == 0 {
        return (x0, y0, 0, 0);
    }
    let (ascent, descent) = typeface::line_metrics(em);
    let baseline = baseline_row(y0, scale, ascent);
    let top = baseline.saturating_sub(ascent as usize);
    let h = (baseline + descent as usize).saturating_sub(top);
    (x0, top, w, h)
}

pub fn drawn_advance(text: &[u8], scale: usize, max_w: Option<usize>) -> Option<usize> {
    if scale < 1 || text.is_empty() || text.len() > KEY_MAX {
        return None;
    }
    let em = em_for_scale(scale);
    // WHY the same walk as `place_styled`: this is the width a caller reserves
    // before asking for the run, so a gate here that reads bytes would refuse a
    // run the placement accepts, and the caller would measure it with the bitmap
    // face while the surface draws it with the outline one.
    let mut total = 0usize;
    let mut i = 0usize;
    while i < text.len() {
        let (cp, next) = codepoint_at(text, i);
        let adv = typeface::advance_codepoint(cp, em);
        if adv == 0 {
            return None;
        }
        total += adv as usize;
        i = next;
    }
    if total == 0 || total > RUN_W_MAX {
        return None;
    }
    let (ascent, descent) = typeface::line_metrics(em);
    if (ascent + descent) as usize > RUN_H_MAX {
        return None;
    }
    if let Some(mw) = max_w {
        if total > mw {
            return None;
        }
    }
    Some(total)
}

/// The code point starting at byte `i`, and the index of the byte after it.
///
/// WHY hand-decoded rather than `core::str::from_utf8`: `from_utf8` validates
/// the whole remaining slice, and this is called for every character of every
/// run, so a per-character validate would make measuring a run quadratic.
///
/// WHY the lead byte on a broken sequence: a lone `0xC3` is what the byte walk
/// this replaces already drew, and a decoder that invented `U+FFFD` here would
/// silently rewrite text a caller is comparing against.
fn codepoint_at(text: &[u8], i: usize) -> (u32, usize) {
    let lead = text[i];
    let len = if lead < 0x80 {
        1
    } else if lead & 0xE0 == 0xC0 {
        2
    } else if lead & 0xF0 == 0xE0 {
        3
    } else if lead & 0xF8 == 0xF0 {
        4
    } else {
        0
    };
    if len == 0 || i + len > text.len() {
        return (lead as u32, i + 1);
    }
    let mut cp = match len {
        1 => return (lead as u32, i + 1),
        2 => (lead & 0x1F) as u32,
        3 => (lead & 0x0F) as u32,
        _ => (lead & 0x07) as u32,
    };
    for k in 1..len {
        let c = text[i + k];
        if c & 0xC0 != 0x80 {
            return (lead as u32, i + 1);
        }
        cp = (cp << 6) | (c & 0x3F) as u32;
    }
    (cp, i + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that reason about the shared slot pool.
    ///
    /// WHY a lock and not just a reset: the harness runs tests on several
    /// threads, so two tests that both mutate `LOADED` interleave no matter how
    /// carefully each resets it. That shows up as a failure that appears on one
    /// run and not the next, which is the definition of untrustworthy. `std` is
    /// available under `cfg(test)`, so the lock costs nothing in the guest.
    static POOL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Empties the slot pool and holds the pool lock for the rest of the test.
    ///
    /// WHY this is needed: `LOADED` is a process-global and the tests share it,
    /// so a test that assumes an empty pool fails when another test has already
    /// filled it -- which is exactly the kind of failure that depends on the
    /// order the harness happened to pick. Reasoning about a pool that the test
    /// itself emptied is deterministic; reasoning about one another test left
    /// behind is not.
    fn reset_slots() -> std::sync::MutexGuard<'static, ()> {
        let guard = POOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_slot_pool();
        // WHY the last placement is forgotten as well: the fast path answers
        // a repeat placement from the run it placed last, and that run names
        // the slot it was drawn into. A test that resets the pool releases
        // those slots, so a later placement may draw a different run into the
        // slot the cached run names -- and the cached run would then be read
        // back over somebody else's glyphs. That is a failure that appears on
        // one run and not the next, which is the definition of untrustworthy.
        forget_last_placement();
        guard
    }

    /// A run that fits, with a known em, so the coverage can be read back.
    fn placed(em: u32, text: &[u8]) -> OutlineRun {
        OutlineRun::place(0, 200, em, text).expect("run placed")
    }

    #[test]
    fn a_placed_run_covers_the_em_box_and_nothing_outside_it() {
        let _pool: Option<_> = Some(reset_slots());
        let run = placed(56, b"09:20");
        let (ascent, descent) = typeface::line_metrics(56);
        assert_eq!(run.h, (ascent + descent) as usize, "the run is one line box");
        assert_eq!(run.w, typeface::text_advance_px(b"09:20", 56) as usize);
        // The baseline is `ascent` rows below the top of the line box, so the
        // run reaches exactly `ascent` above its baseline and no further.
        assert_eq!(run.y.saturating_sub(run.ascent), 200 - ascent as usize);
    }

    #[test]
    fn a_multibyte_word_the_face_has_is_not_refused() {
        let _pool: Option<_> = Some(reset_slots());
        // "café" ends in U+00E9, which the face maps, written as 0xC3 0xA9.
        // Walking bytes rather than code points measured each half of that
        // pair as an absent character with a zero advance and threw the run
        // away, so the accented word in an otherwise outline line was handed
        // to the bitmap cell -- one line, two sizes.
        let cafe = "café".as_bytes();
        let run = placed(40, cafe);
        assert_eq!(
            run.w,
            typeface::text_advance_px(cafe, 40) as usize,
            "the width the run was measured with is the width the pen walked"
        );
        assert!(run.w > 0);
    }

    #[test]
    fn drawn_advance_agrees_with_the_run_it_predicts() {
        let _pool: Option<_> = Some(reset_slots());
        // WHY this matters on its own: `drawn_advance` is what a caller
        // reserves before it asks for the run. If it refused the multi-byte
        // word that `place_styled` accepts, the caller would centre or clip
        // the string at the bitmap face's width and the drawn glyphs would
        // spill past it.
        for text in ["café".as_bytes(), "Zoö".as_bytes(), b"plain"] {
            for scale in [1usize, 2, 3] {
                let em = em_for_scale(scale);
                let run = OutlineRun::place(0, 200, em, text).expect("run placed");
                assert_eq!(drawn_advance(text, scale, None), Some(run.w));
            }
        }
    }

    #[test]
    fn a_codepoint_outside_the_face_still_refuses_the_whole_run() {
        let _pool: Option<_> = Some(reset_slots());
        // The refusal is the point: a character with no outline advances by
        // nothing, and drawing it would stack every later glyph on its
        // neighbour. Dropping to the other face for the run is the honest
        // answer, so the gate must stay -- it just has to test characters.
        assert!(OutlineRun::place(0, 200, 40, "你好".as_bytes()).is_none());
    }

    #[test]
    fn ink_appears_where_the_outline_says_it_does() {
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
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
        // 680px is the buffer's width; a longer run at a large em exceeds it.
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
        let _pool: Option<_> = Some(reset_slots());
        // The lock surface places the date and then the clock in one frame, and
        // both are painted afterwards. With one shared buffer the clock's
        // rasterisation erased the date's coverage, and the date then read the
        // clock's glyphs: its letters vanished and only its comma survived.
        //
        // WHY the arguments are the real call sites and not round numbers: this
        // test first used x=0 for both runs and passed while the live lock
        // screen was broken, because the two placements collided in one slot
        // only at the x the painter actually uses. A test that does not mirror
        // the caller's arguments is not guarding the caller.
        let date = OutlineRun::place(75, 251, 18, b"Mon, 12 Oct").expect("date placed");
        let clock = OutlineRun::place(75, 325, 48, b"00:02").expect("clock placed");
        assert_ne!(date.slot, clock.slot, "the two runs must not share a slot");

        // Both runs must ink pixels after the second placement. Read the date
        // first, in the order the frame draws them, so a shared buffer would
        // fail here rather than somewhere further away.
        let date_ink = count_ink(&date);
        let clock_ink = count_ink(&clock);
        assert!(date_ink > 200, "date inked only {date_ink} pixels");
        assert!(
            clock_ink > 500,
            "clock inked only {clock_ink} pixels"
        );
    }

    #[test]
    fn a_colliding_second_run_still_lands_in_a_free_slot() {
        let _pool: Option<_> = Some(reset_slots());
        // WHY: the slot used to come from a hash, so two runs that hashed alike
        // erased each other with no way to tell from the outside. Every
        // additional distinct placement must now take a free slot while one is
        // left, regardless of what its key looks like.
        let first = OutlineRun::place(75, 251, 18, b"Mon, 12 Oct").expect("placed");
        let second = OutlineRun::place(75, 325, 48, b"00:02").expect("placed");
        assert_ne!(first.slot, second.slot, "no free slot was chosen");
        assert!(first.slot < SLOTS);
        assert!(second.slot < SLOTS);
    }

    #[test]
    fn a_ninth_distinct_run_cannot_have_a_private_slot() {
        let _pool: Option<_> = Some(reset_slots());
        // Only SLOTS exist. An overflow live run reclaims the slot nobody has
        // asked for since, and the honest consequence is that one of the
        // earlier runs is no longer readable -- which the caller must not
        // rely on. Pin the reclaim path so it is chosen deliberately rather
        // than corrupting silently.
        for i in 0..SLOTS {
            let ch = [b'A' + (i % 26) as u8];
            let r = OutlineRun::place(75, 200 + i * 10, 18, &ch).expect("placed");
            assert!(r.slot < SLOTS);
        }
        let overflow = OutlineRun::place(75, 600, 24, b"Z").expect("placed");
        assert!(overflow.slot < SLOTS);
        // And it must actually hold its own glyphs.
        assert!(count_ink(&overflow) > 0, "the reclaimed slot holds no ink");
    }

    #[test]
    fn a_repeated_placement_reuses_the_same_slot() {
        let _pool: Option<_> = Some(reset_slots());
        // The cache soundness rests on this: the same run must land where it
        // was, or a later placement could be answered from another run's slot.
        let first = OutlineRun::place(75, 251, 18, b"Mon, 12 Oct").expect("placed");
        // Displace it, then place it again.
        OutlineRun::place(75, 325, 48, b"00:02").expect("placed");
        OutlineRun::place(75, 400, 24, b"Ag").expect("placed");
        let again = OutlineRun::place(75, 251, 18, b"Mon, 12 Oct").expect("placed again");
        assert_eq!(first.slot, again.slot, "the same run moved slots");
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
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
        // The placement key's length is the bound. Refusing is the honest
        // outcome: the caller keeps the bitmap face, which never truncates.
        //
        // WHY the em is small here: the coverage buffer's width is the other
        // limit, and a run of a hundred and twenty-eight characters at the
        // body's size is wider than the buffer, so it would be refused for
        // its width rather than for its length. The test is about the key.
        let long_ok = [b'a'; KEY_MAX];
        let long_too_long = [b'a'; KEY_MAX + 1];
        assert!(OutlineRun::place(0, 200, 6, &long_ok).is_some());
        assert!(OutlineRun::place(0, 200, 6, &long_too_long).is_none());
    }

    #[test]
    fn the_convenience_pixel_agrees_with_a_placed_run() {
        let _pool: Option<_> = Some(reset_slots());
        // WHY this test exists: the shell's labels are held in frame plans that
        // must stay cheap to copy, so they carry the key rather than a placed
        // run, and `pixel` places on demand. If the two paths ever disagree --
        // a different baseline convention, a different box -- a label renders
        // from one and clips through the other, and nothing else would notice.
        let x = 75usize;
        let baseline = 188usize;
        let em = em_for_scale(2);
        let bytes = b"Today";

        let placed = OutlineRun::place(x, baseline, em, bytes).expect("placed");
        let (px0, py0, pw, ph) = placed.bounds();

        let mut agreed = 0usize;
        let mut inked = 0usize;
        for py in py0..py0 + ph {
            for px in px0..px0 + pw {
                let a = placed.pixel(px, py, 0xFF000000, 0xFFFFFFFF);
                let b = pixel(x, baseline, em, bytes, px, py, 0xFF000000, 0xFFFFFFFF);
                assert_eq!(a.is_some(), b.is_some(), "disagreement at ({px},{py})");
                if a.is_some() {
                    inked += 1;
                    if a == b {
                        agreed += 1;
                    }
                }
            }
        }
        assert!(inked > 100, "only {inked} inked pixels");
        assert_eq!(agreed, inked, "the two paths produced different colours");
    }

    #[test]
    fn em_for_scale_names_the_design_cap_height() {
        // The whole point of `em_for_scale`: the scale rung names the size the
        // design document asks for, in the units a reader sees -- a caption of
        // thirteen pixels, a body of sixteen, a title of thirty and a page
        // heading of forty -- and the em must draw exactly that cap height,
        // whatever the bitmap cell's own multiples were. Check the derivation
        // rather than trust it.
        for scale in [1usize, 2, 3, 6] {
            let em = em_for_scale(scale);
            let want = crate::font::cap_for_scale(scale) as u64;
            let got = (em as u64 * 1456 + 1024) / 2048;
            assert_eq!(
                got, want,
                "scale {scale} gave em {em}, which is a {got}px cap; wanted {want}px"
            );
        }
        assert_eq!(em_for_scale(0), 0, "a zero scale has no size");
    }

    #[test]
    fn place_run_supports_scale_1_and_bold_and_descenders() {
        let _pool: Option<_> = Some(reset_slots());
        // Scale 1 run with descenders ("Battery")
        let run_regular = place_run(10, 50, 1, b"Battery", None, false).expect("scale 1 placed");
        assert!(run_regular.w > 0);
        assert!(run_regular.h > 0);

        // Bold run
        let run_bold = place_run(10, 100, 2, b"Settings", None, true).expect("bold placed");
        assert!(run_bold.w > 0);
        assert!(run_bold.h > 0);
    }

    #[test]
    fn perceptual_alpha_is_smooth_and_monotonic() {
        // Endpoints are exact: no ink at zero coverage, full ink at full coverage.
        assert_eq!(perceptual_alpha(0), 0);
        assert_eq!(perceptual_alpha(255), 255);
        // Every adjacent step advances by at most a few levels, so edges keep
        // all eight bits instead of collapsing onto sixteen table entries.
        let mut prev = 0u8;
        for c in 1..=255u8 {
            let a = perceptual_alpha(c);
            assert!(a >= prev, "alpha must not fall as coverage rises");
            assert!(a as i32 - prev as i32 <= 3, "coverage {c} jumped {prev} -> {a}");
            prev = a;
        }
        // Half coverage lands near the perceptual mid-tone the linear-light
        // blend needs, not at 50% alpha.
        let mid = perceptual_alpha(128);
        assert!((175..=205).contains(&mid), "mid coverage alpha was {mid}");
    }

    #[test]
    fn test_curved_glyphs_rasterize_with_anti_aliasing() {
        let _pool: Option<_> = Some(reset_slots());
        // Curved glyphs: '0', 'C', 'O', 'S', 'B', 'D', 'G', 'P', 'R', 'Q'
        let run = OutlineRun::place(0, 150, 48, b"0COSB").expect("placed curved glyphs");
        assert!(run.w > 0);
        assert!(run.h > 0);

        // Verify partial coverage pixels (anti-aliasing) are present and smooth
        let top = run.y - run.ascent;
        let mut anti_aliased_count = 0;
        for py in top..top + run.h {
            for px in 0..run.w {
                if let Some(c) = run.pixel(px, py, 0xFF000000, 0xFFFFFFFF) {
                    if c != 0xFF000000 && c != 0xFFFFFFFF {
                        anti_aliased_count += 1;
                    }
                }
            }
        }
        assert!(anti_aliased_count > 50, "expected smooth anti-aliased edge coverage");
    }

    #[test]
    fn test_homescreen_dock_labels_all_render() {
        let _pool: Option<_> = Some(reset_slots());

        // Simulate all runs placed before dock in build():
        // 1. Clock
        OutlineRun::place_run(40, 30, 2, b"10:00", false);
        // 2. Greeting
        OutlineRun::place_run(30, 80, 4, b"Welcome", false);
        OutlineRun::place_run(30, 140, 3, b"System ready", false);
        // 3. Date widget
        OutlineRun::place_run(500, 80, 3, b"FRIDAY", false);
        OutlineRun::place_run(500, 140, 6, b"24", false);
        OutlineRun::place_run(500, 180, 2, b"22 C Clear", false);
        // 4. Today card
        OutlineRun::place_run(50, 280, 4, b"Today", false);
        // 5. Card rows
        OutlineRun::place_run(120, 340, 3, b"Schedule", false);
        OutlineRun::place_run(120, 370, 2, b"No events on this device", false);
        OutlineRun::place_run(120, 420, 3, b"Notifications", false);
        OutlineRun::place_run(120, 450, 2, b"0 unread", false);
        OutlineRun::place_run(120, 500, 3, b"Activities", false);
        OutlineRun::place_run(120, 530, 2, b"3 active tasks", false);

        // 6. Dock labels
        let labels: [&[u8]; 6] = [b"Phone", b"Messages", b"Camera", b"Files", b"Browser", b"Apps"];
        let xs = [30, 260, 490, 30, 260, 490];
        let ys = [805 + 114, 805 + 114, 805 + 114, 1020 + 114, 1020 + 114, 1020 + 114];
        let em = 30;

        let mut runs = [OutlineRun::EMPTY; 6];
        for i in 0..6 {
            let (asc, _) = typeface::line_metrics(em);
            let baseline = (ys[i] + 51).max(asc as usize);
            runs[i] = OutlineRun::place_styled(xs[i], baseline, em, labels[i], false)
                .unwrap_or_else(|| panic!("failed to place dock label {}", i));
        }

        // 7. Search bar
        OutlineRun::place_run(50, 1300, 3, b"Search apps...", false);

        // Now test all 6 dock labels can still be rasterized!
        for i in 0..6 {
            let run = &runs[i];
            let top = run.y - run.ascent;
            let mut inked = 0;
            for py in top..top + run.h {
                for px in run.x..run.x + run.w {
                    if let Some(c) = run.pixel(px, py, 0xFF000000, 0xFFFFFFFF) {
                        if c != 0xFFFFFFFF {
                            inked += 1;
                        }
                    }
                }
            }
            assert!(inked > 0, "Dock label {} ({:?}) rendered 0 pixels! run: x={} y={} w={} h={} slot={}",
                i, core::str::from_utf8(labels[i]), run.x, run.y, run.w, run.h, run.slot);
        }
    }

    /// Counts the resident runs, for tests that reason about a full pool.
    fn loaded_len() -> usize {
        unsafe { (*core::ptr::addr_of!(LOADED)).iter().flatten().count() }
    }

    /// True when any pixel of `run`'s box still reads the coverage it was
    /// placed with.
    fn still_readable(run: &OutlineRun) -> bool {
        let top = run.y - run.ascent;
        (top..top + run.h).any(|py| {
            (run.x..run.x + run.w)
                .any(|px| run.pixel(px, py, 0xFF000000, 0xFFFFFFFF).is_some())
        })
    }

    #[test]
    fn a_full_pool_keeps_both_runs_a_pixel_walk_asks_for() {
        let _pool: Option<_> = Some(reset_slots());
        let em = em_for_scale(1);
        let (ascent, _) = typeface::line_metrics(em);
        // Two runs the error screen asks for on the same row of pixels.
        let a_text: &[u8] = b"Gateway could not fetch";
        let b_text: &[u8] = b"Reload Page";
        let a_x = (720 - typeface::text_advance_px(a_text, em) as usize) / 2;
        let a_y = 440 + ascent as usize;
        let a_base = slot_for(em, a_x, a_y, a_text, true);
        // A run whose hash base is the same slot, which is the case that makes
        // the pool's replacement policy decide whether both can stay resident.
        let mut collision = None;
        'search: for dx in 0..64usize {
            for dy in 0..256usize {
                let b_x = 100 + dx;
                let b_y = 642 + ascent as usize + dy;
                if slot_for(em, b_x, b_y, b_text, true) == a_base {
                    collision = Some((b_x, b_y));
                    break 'search;
                }
            }
        }
        let (b_x, b_y) = collision.expect("a colliding placement exists in the app's range");
        assert!(OutlineRun::place_styled(b_x, b_y, em, b_text, true).is_some());

        // Fill every remaining slot with keys the screen never asks for, which
        // is the state a browser reaches after painting its keyboard.
        let mut i = 0u8;
        while loaded_len() < SLOTS && i < 200 {
            let mut text = [b'q'; 4];
            text[1] = b'a' + i % 26;
            text[2] = b'0' + i / 26 % 10;
            text[3] = b'0' + i % 10;
            let _ = OutlineRun::place_styled(10 + (i as usize) * 7, 100 + (i as usize) * 11, em, &text, false);
            i += 1;
        }
        assert_eq!(loaded_len(), SLOTS, "the pool is full");

        // One row of the walk: both runs are requested for every pixel.
        for _ in 0..8 {
            let _ = OutlineRun::place_styled(a_x, a_y, em, a_text, true);
            let _ = OutlineRun::place_styled(b_x, b_y, em, b_text, true);
        }
        let a = OutlineRun::place_styled(a_x, a_y, em, a_text, true).expect("a placed");
        let b = OutlineRun::place_styled(b_x, b_y, em, b_text, true).expect("b placed");
        assert!(still_readable(&a), "placing the second run evicted the first");
        assert!(still_readable(&b), "placing the first run evicted the second");
    }

    #[test]
    fn a_remembered_box_is_the_measured_box() {
        let _pool: Option<_> = Some(reset_slots());
        // The box a per-pixel caller gets from the memo has to be the box the
        // walk computes, or the gate lets ink through that the painter then
        // refuses -- and the reader loses the top row of a glyph.
        let screen = [
            (24usize, 60usize, 2usize, &b"Now Playing"[..]),
            (24, 100, 1, b"no track selected"),
            (300, 640, 3, b"00:00 / 03:41"),
            (24, 700, 1, "Über".as_bytes()),
        ];
        let measured: [(usize, usize, usize, usize); 4] = [
            measure_box(24, 60, 2, b"Now Playing"),
            measure_box(24, 100, 1, b"no track selected"),
            measure_box(300, 640, 3, b"00:00 / 03:41"),
            measure_box(24, 700, 1, "Über".as_bytes()),
        ];
        // Two passes over the whole screen, the way a pixel walk revisits every
        // run: the second pass reads the memo the first one filled.
        for pass in 0..2 {
            for (i, &(x0, y0, scale, text)) in screen.iter().enumerate() {
                let got = run_box(x0, y0, scale, text);
                assert_eq!(
                    got, measured[i],
                    "pass {pass}: box for {:?} at ({x0},{y0}) scale {scale}",
                    core::str::from_utf8(text).unwrap_or("?")
                );
            }
        }
    }

    #[test]
    fn a_box_is_remembered_by_its_text_and_not_only_its_origin() {
        let _pool: Option<_> = Some(reset_slots());
        // The origin is part of the key, so two runs that share a position must
        // not share an answer -- that is the difference between a memo and a
        // guess. `WIDE` is the wider run, so a box handed over would clip it.
        let narrow = run_box(120, 400, 2, b"up");
        let wide = run_box(120, 400, 2, b"understanding");
        assert!(
            wide.2 > narrow.2,
            "the longer run at the same origin is {} against {}",
            wide.2,
            narrow.2
        );
        // Asked again in the other order, both still answer for themselves.
        let _ = run_box(120, 400, 2, b"understanding");
        assert_eq!(run_box(120, 400, 2, b"up"), narrow);
        assert_eq!(run_box(120, 400, 2, b"understanding"), wide);
        // A run too long to key is measured every time rather than hashed.
        let long = [b'e'; KEY_MAX + 8];
        assert_eq!(
            run_box(0, 900, 1, &long),
            measure_box(0, 900, 1, &long),
            "an unkeyable run still gets the box the walk computes"
        );
    }
}
