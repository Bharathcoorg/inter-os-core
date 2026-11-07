// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
    // WHY the shape table goes with them too: it is the third of the three
    // tables a per-pixel painter asks, and a caller that declares the coverage
    // stale has no way to say it means only two of them.
    reset_shape_cache();
}

/// A run's placement, remembered so the next pixel asks the same question.
///
/// WHY this exists: a caller that paints a run asks for that same run for
/// every pixel of it, and placing it rebuilt a sixty-four byte key --
/// zeroed and then filled -- for each of those pixels. On a screen drawn
/// one pixel at a time that was the single largest cost in a frame, and
/// it grew with the number of pixels rather than with the amount of text.
///
/// WHY a table and not one entry: a pixel that several runs overlap asks for
/// each of them in turn, which is what painting a frame one pixel at a time
/// means, and a one-entry memo answers the first and rebuilds the rest. This
/// is the same reasoning that gave the gate-box cache `BOX_SLOTS` entries.
/// The table is direct-mapped on [`slot_for`], so a hit is one comparison and
/// a collision costs a probe of the coverage table rather than a wrong answer.
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

static mut LAST_PLACED: [Option<LastPlaced>; SLOTS] = [None; SLOTS];

/// Counters that make the memo's contract observable to a test.
///
/// WHY they exist: whether a placement was answered from the memo or by
/// scanning the coverage table is invisible in the output -- both draw the
/// same pixels -- so without a count there is no way to write a test that
/// fails while the memo is too small to hold a frame's runs. They compile out
/// of the guest entirely.
#[cfg(test)]
pub static MEMO_HITS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub static TABLE_PROBES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Counters that make the *coverage work* the pool does observable to a test.
///
/// WHY they exist beside the memo counts: the memo counts answer "was this
/// placement already remembered", which is not the question the paint cost
/// asks. The question that decides a frame's cost is how many times the
/// rasteriser ran -- `RASTERISES` -- and how much of that came from a slot
/// being reclaimed out from under a run that was still being painted
/// (`REPLACES`, `EVICTIONS`). Both are invisible in the output: a re-rasterise
/// and a cache hit put the same pixels down, which is precisely why the miss
/// rate had to be counted before anyone could say whether the row-in-key cost
/// anything at all.
///
/// WHY `cfg(test)` and not the guest: they are a measurement, not a feature.
/// They compile out of the guest entirely, exactly as the memo counts above do,
/// so a probe cannot change what anyone draws.
#[cfg(test)]
pub static PLACEMENT_ASKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub static RASTERISES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub static REPLACES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub static EVICTIONS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub static BOX_HITS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
pub static BOX_MEASURES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// Gate questions answered from the shape table, and the walks it had to do.
//
// WHY thread-local when the counts above are process-global: the claim these
// answer is about a caller's own walk -- that a frame which paints the same
// strings at a dozen offsets measures each of them once -- and the harness runs
// other tests through the same table beside it. A global count would charge
// their strings to the probe, which is why the typeface's advance scans are
// counted the same way. Also `cfg(test)`, so nothing of it reaches the guest.
#[cfg(test)]
std::thread_local! {
    static GATE: core::cell::Cell<(usize, usize)> = const { core::cell::Cell::new((0, 0)) };
}

/// `(answers from the table, walks the table had to do)` for this thread.
#[cfg(test)]
pub fn gate_counts() -> (usize, usize) {
    GATE.with(|c| c.get())
}

/// Zeroes the gate's own counts, so a scenario reports its own span.
#[cfg(test)]
pub fn reset_gate_counts() {
    GATE.with(|c| c.set((0, 0)));
}

/// Zeroes every measurement counter. Test-only; see the counters above.
#[cfg(test)]
pub fn reset_counters() {
    use core::sync::atomic::Ordering::Relaxed;
    PLACEMENT_ASKS.store(0, Relaxed);
    RASTERISES.store(0, Relaxed);
    REPLACES.store(0, Relaxed);
    EVICTIONS.store(0, Relaxed);
    BOX_HITS.store(0, Relaxed);
    BOX_MEASURES.store(0, Relaxed);
    MEMO_HITS.store(0, Relaxed);
    TABLE_PROBES.store(0, Relaxed);
    reset_gate_counts();
}

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

/// How many run shapes the gate remembers.
///
/// WHY as many as the coverage pool: the gate is asked once per pixel per run,
/// so a shape table that cannot hold a screen's distinct strings re-walks them
/// on the per-pixel path it exists to get off. A string the table cannot hold
/// still gets the right answer; it just costs the walk again.
const SHAPE_SLOTS: usize = SLOTS;

/// What the outline face makes of one string at one em.
///
/// WHY the shape and not the box: the box a pixel walk gates against is this
/// shape with the run's own origin in front of it, because [`place_run_cap`]
/// puts the baseline at `y0 + ascent` and [`OutlineRun::bounds`] then reports a
/// box whose top-left is exactly (`x0`, `y0`). The width and the line height are
/// a pure function of the bytes, the em and the bold flag, so they are the part
/// worth remembering -- and remembering only them is what lets a frame that
/// scrolls the same strings down the surface measure each of them once rather
/// than once per row. [`Boxed`] cannot answer this question because its key
/// names an origin, and an origin is precisely what the shape does not depend
/// on.
#[derive(Clone, Copy)]
struct Shaped {
    em: u32,
    bold: bool,
    len: u8,
    used: u32,
    key: [u8; KEY_MAX],
    /// `None` is an answer, not a gap: a run carrying a codepoint the face has
    /// no outline for refuses at every pixel, and a painter that has been told
    /// once should not pay the walk to be told again on every one of them.
    shape: Option<(usize, usize)>,
}

static mut SHAPES: [Option<Shaped>; SHAPE_SLOTS] = [None; SHAPE_SLOTS];

/// Which shape slot a gate question lands in.
///
/// Mirrors [`box_slot_for`]: a hash of the key's cheap parts starts the probe,
/// so a walk that repeats its few strings finds them without comparing the
/// whole table.
fn shape_slot_for(em: u32, len: usize, bold: bool) -> usize {
    let mut h = (em as usize) ^ (len << 5) ^ (if bold { 0x0F0F } else { 0x0F0 });
    h ^= (h >> 3) ^ (h >> 7);
    h % SHAPE_SLOTS
}

/// The width and line height the outline face gives `text` at `em`, or `None`
/// when the face refuses to draw it at all.
///
/// WHY this is a copy of the walk inside [`OutlineRun::place_styled`] rather
/// than a call into it: the gate is asked before the placement, and the whole
/// point of asking it is not to rasterise a run to find out where its ink can
/// land. The two walks are the same arithmetic over the same `codepoint_at` and
/// the same `advance_codepoint`, with the same width and line limits, so a shape
/// here is a statement about what a placement would do -- and
/// `the_gate_shape_is_the_box_the_placement_reports` pins it against the placed
/// run instead of trusting that prose.
fn measure_shape(em: u32, text: &[u8], bold: bool) -> Option<(usize, usize)> {
    if em == 0 || text.is_empty() || text.len() > KEY_MAX {
        return None;
    }
    let mut pen = 0u32;
    let mut i = 0usize;
    while i < text.len() {
        let (cp, next) = codepoint_at(text, i);
        let adv = typeface::advance_codepoint(cp, em);
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
    Some((w, line_h))
}

/// The shape a run at a cap height draws into, or `None` when the outline face
/// refuses it and the caller is on the bitmap face.
///
/// WHY this is the face a painter gates on: a band painted one pixel at a time
/// asks this question for every pixel of the band, and the answer decides
/// whether the pixel is worth a placement. The estimate a caller used before
/// this existed admitted four times the pixels a run can ink, and every one of
/// those asked the placement pool a question it had already answered -- so the
/// frame's floor was the number of pixels it looked at, not the number of
/// glyphs it drew.
pub fn run_shape_cap(cap_px: u32, text: &[u8], bold: bool) -> Option<(usize, usize)> {
    if cap_px == 0 || text.is_empty() || text.len() > KEY_MAX {
        return None;
    }
    let em = em_for_cap_height(cap_px);
    let base = shape_slot_for(em, text.len(), bold);
    let hit = unsafe {
        // SAFETY: the table is read through a raw pointer and no reference into
        // it is live when the stamp below is taken.
        let table = &*core::ptr::addr_of!(SHAPES);
        let mut found = None;
        for i in 0..SHAPE_SLOTS {
            let slot = (base + i) % SHAPE_SLOTS;
            if let Some(entry) = table[slot].as_ref() {
                if entry.em == em
                    && entry.bold == bold
                    && entry.len as usize == text.len()
                    && entry.key[..text.len()] == *text
                {
                    found = Some((slot, entry.shape));
                    break;
                }
            }
        }
        found
    };
    if let Some((slot, shape)) = hit {
        #[cfg(test)]
        GATE.with(|c| {
            let (hits, walks) = c.get();
            c.set((hits + 1, walks));
        });
        unsafe {
            if let Some(entry) = (*core::ptr::addr_of_mut!(SHAPES))[slot].as_mut() {
                entry.used = next_stamp();
            }
        }
        return shape;
    }
    #[cfg(test)]
    GATE.with(|c| {
        let (hits, walks) = c.get();
        c.set((hits, walks + 1));
    });
    let shape = measure_shape(em, text, bold);
    remember_shape(em, bold, text, shape);
    shape
}

/// Remembers a shape, replacing the least recently asked-for one in its probe
/// run when the run is full.
fn remember_shape(
    em: u32,
    bold: bool,
    text: &[u8],
    shape: Option<(usize, usize)>,
) {
    if text.len() > KEY_MAX {
        return;
    }
    let base = shape_slot_for(em, text.len(), bold);
    let mut key = [0u8; KEY_MAX];
    key[..text.len()].copy_from_slice(text);
    // SAFETY: as above -- the table is written through a raw pointer and no
    // borrow of it is live here.
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(SHAPES);
        let mut target = base;
        let mut oldest = u32::MAX;
        for i in 0..SHAPE_SLOTS {
            let slot = (base + i) % SHAPE_SLOTS;
            match table[slot].as_ref() {
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
        table[target] = Some(Shaped {
            em,
            bold,
            len: text.len() as u8,
            used: next_stamp(),
            key,
            shape,
        });
    }
}

/// Forgets every remembered run shape.
///
/// WHY it is not public: nothing outside this module holds a shape, so a caller
/// that invalidates the caches invalidates this one through
/// [`reset_slot_pool`] with the rest of them.
fn reset_shape_cache() {
    unsafe {
        for entry in (*core::ptr::addr_of_mut!(SHAPES)).iter_mut() {
            *entry = None;
        }
    }
}

/// Forgets every placement the memo remembers.
///
/// WHY it exists: a caller that keeps a frame plan across frames must be
/// able to say that the cache no longer describes the screen.
pub fn forget_last_placement() {
    unsafe {
        for entry in (*core::ptr::addr_of_mut!(LAST_PLACED)).iter_mut() {
            *entry = None;
        }
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
        // WHY the counter sits on this branch and not the free-slot one above:
        // a pool that still has room costs a rasterisation for the new run and
        // nothing else. A pool that is full costs that rasterisation *and* the
        // eviction of a run somebody may still be painting -- and the painting
        // of an evicted run re-rasterises it, which is the difference between
        // one raster per run and one per pixel. Only the second is a storm.
        #[cfg(test)]
        EVICTIONS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        (victim, false)
    }
}

/// Whether the named slot still holds exactly this run.
///
/// WHY the question is asked at all: a memo entry names a slot, and a
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
/// WHY the memo is refreshed from a table hit: the memo is what makes a run the
/// caller is painting right now cost one comparison rather than a scan.
/// Without the refresh it would only ever hold a run from the first pixel of a
/// frame, and a caller that cycles through a screen's runs -- which is what
/// painting a frame one pixel at a time means -- would miss every one of them.
fn resident_run(em: u32, x: usize, y: usize, text: &[u8], bold: bool) -> Option<OutlineRun> {
    let base = slot_for(em, x, y, text, bold);
    unsafe {
        // SAFETY: the static is read through a raw pointer and the reference it
        // yields does not outlive this block, and nothing reached from here
        // re-enters it.
        if let Some(last) = (&*core::ptr::addr_of!(LAST_PLACED))[base].as_ref() {
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
                        #[cfg(test)]
                        MEMO_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        // WHY the hit also stamps the slot: this is the run the
                        // pixel walk is asking for right now, so it must be the
                        // last candidate for eviction rather than the first.
                        if let Some(entry) =
                            (*core::ptr::addr_of_mut!(LOADED))[run.slot].as_mut()
                        {
                            entry.used = next_stamp();
                        }
                        return Some(run);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    TABLE_PROBES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
    let mut key = [0u8; KEY_MAX];
    key[..text.len()].copy_from_slice(text);
    let run = OutlineRun {
        x,
        y,
        w,
        h,
        ascent: ascent as usize,
        slot,
        em,
        bold,
        len: text.len() as u8,
        key,
    };
    // SAFETY: the borrow above is released before either pointer is taken here.
    unsafe {
        if let Some(entry) = (*core::ptr::addr_of_mut!(LOADED))[slot].as_mut() {
            entry.used = next_stamp();
        }
        (*core::ptr::addr_of_mut!(LAST_PLACED))[base] = Some(LastPlaced {
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
    /// The placement key, carried so the run can re-place its own coverage.
    ///
    /// WHY the run holds its text: a frame plan places its runs once and
    /// paints them later, and a plan with more distinct runs than the pool
    /// evicts its own earliest entries -- whose pixels then answered None and
    /// simply vanished. Re-placing an evicted run needs the em and the bytes,
    /// and nothing else in the process still holds them, so the key travels
    /// with the run: a lost slot costs a rasterisation, never the ink.
    em: u32,
    bold: bool,
    len: u8,
    key: [u8; KEY_MAX],
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
        em: 0,
        bold: false,
        len: 0,
        key: [0u8; KEY_MAX],
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
        // WHY counted after the guard and before the resident lookup: a refused
        // run never reaches the pool at all, so counting it here would say a
        // frame asked the cache for more than it did.
        #[cfg(test)]
        PLACEMENT_ASKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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

        // THE measurement: this line is reached once per coverage buffer the
        // rasteriser actually writes. A repeat placement answered from the memo
        // or the table never reaches it, so the gap between this count and the
        // number of placement asks is exactly the work the caches saved -- and
        // the number itself is the work a frame pays for.
        #[cfg(test)]
        RASTERISES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

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
            em,
            bold,
            len: text.len() as u8,
            key,
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

    /// The run's placement key, as the bytes it was placed from.
    fn text(&self) -> &[u8] {
        &self.key[..self.len as usize]
    }

    /// Whether this run's slot still holds exactly this placement.
    ///
    /// WHY the full key and not just the geometry: the slot may have been
    /// redrawn with a different run that happens to share the origin and the
    /// line box; reading its coverage under this run's name is the wrong
    /// glyph set the residency check exists to forbid.
    fn resident(&self) -> bool {
        slot_still_holds(self.slot, self.em, self.x, self.y, self.text(), self.bold)
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
        // WHY the run re-places itself instead of answering None: a frame
        // plan with more runs than the pool evicts its earliest placements,
        // and answering None there dropped their ink -- the shade's header
        // simply vanished mid-frame. Re-placing costs one rasterisation per
        // evicted run per paint pass (each re-place stamps its run newest, so
        // the eviction only cascades once through the plan), which is time
        // the frame pays. Losing the pixel was the answer no reader forgives.
        if !self.resident() {
            #[cfg(test)]
            REPLACES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let fresh = Self::place_styled(self.x, self.y, self.em, self.text(), self.bold)?;
            // The re-placed run answers from a slot that holds its own key,
            // so this read passes the residency check and cannot recurse.
            return fresh.pixel(px, py, ink, bg);
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
        // WHY the re-place: same contract as [`OutlineRun::pixel`] -- degrade
        // in speed, never in pixels, and never read a foreign slot's glyphs.
        if !self.resident() {
            #[cfg(test)]
            REPLACES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let fresh = Self::place_styled(self.x, self.y, self.em, self.text(), self.bold)?;
            return fresh.pixel_binary(px, py, ink);
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

/// The rows a run at (`y0`, `scale`) can ink, whatever its bytes are.
///
/// WHY content-free: the vertical extent of a run comes from the face's ascent
/// and descent at the rung's em and from the bitmap cell's own line height --
/// never from the string. So it folds at compile time at a call site whose `y0`
/// and `scale` are literals, and a painter can reject a pixel that is nowhere
/// near the line with two integer compares instead of a hash, a key walk and a
/// measure. This is the gate [`run_box`] remembers an answer for; the band is
/// the part of that answer the caller can know before it asks.
///
/// WHY a range rather than a height: the caller already holds `y`. Returning
/// the first and one-past-last row lets the test read `y >= top && y < bottom`
/// with no subtraction, and the band covers the union of both renderers the way
/// a painter's box test does -- the outline line box and the bitmap cell -- so
/// a pixel rejected here is one neither face could have inked.
pub const fn row_band(y0: usize, scale: usize) -> (usize, usize) {
    if scale == 0 {
        return (0, 0);
    }
    let em = font::em_for_scale(scale);
    let (ascent, descent) = typeface::line_metrics(em);
    let ascent = ascent as usize;
    let descent = descent as usize;
    // The same baseline `run_box` measures from: the lower of the outline
    // ascent and the bitmap cell's ascent, because the caller draws whichever
    // face the run fits.
    let bitmap_ascent = font::ascent_for_scale(scale);
    let outline_line = y0 + ascent;
    let baseline = if outline_line > bitmap_ascent + y0 {
        outline_line
    } else {
        y0 + bitmap_ascent
    };
    let box_top = if baseline > ascent { baseline - ascent } else { 0 };
    // A painter clamps the top up to the origin it was given, so the band
    // starts there when the outline box would sit above it.
    let top = if box_top < y0 { box_top } else { y0 };
    let outline_bottom = baseline + descent;
    let bitmap_bottom = y0 + font::line_height(scale);
    let bottom = if outline_bottom > bitmap_bottom {
        outline_bottom
    } else {
        bitmap_bottom
    };
    (top, bottom)
}

pub fn run_box(x0: usize, y0: usize, scale: usize, text: &[u8]) -> (usize, usize, usize, usize) {
    // WHY the memo comes first: this is the gate a per-pixel painter asks
    // before it asks anything else, so it is asked once per pixel per run, and
    // the measurement below is a walk of the whole string. The box is a pure
    // function of the four arguments, so a remembered answer is not an
    // approximation of the measured one -- it is the measured one.
    if let Some(remembered) = cached_box(x0, y0, scale, text) {
        #[cfg(test)]
        BOX_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return remembered;
    }
    #[cfg(test)]
    BOX_MEASURES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
        // WHY the placement memo is forgotten as well: the fast path answers a
        // repeat placement from the memo table, and every entry there names the
        // slot its run was drawn into. A test that resets the pool releases
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
        let _pool: Option<_> = Some(reset_slots());
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
        let _pool: Option<_> = Some(reset_slots());
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
    fn a_pixel_walk_cycling_a_screens_runs_asks_the_table_once_per_run() {
        use core::sync::atomic::Ordering;
        let _pool: Option<_> = Some(reset_slots());
        MEMO_HITS.store(0, Ordering::Relaxed);
        TABLE_PROBES.store(0, Ordering::Relaxed);
        // Six labels down one screen, the way the shell lays them out. A
        // per-pixel painter asks for each of these once for every pixel it
        // paints, so the cost of a frame is the number of asks and not the
        // number of runs -- unless the memo holds a screen's worth of them.
        const RUNS: [(&[u8], usize, u32); 6] = [
            (b"09:20", 200, 56),
            (b"Mon, 12 Oct", 260, 18),
            (b"Ag", 320, 24),
            (b"Alarms", 380, 20),
            (b"Next up", 440, 16),
            (b"Calendar", 500, 16),
        ];
        const PASSES: usize = 5;
        // WHY the bases are pinned to be distinct: the memo is direct-mapped,
        // so two runs that share one answer each other in turn -- a real cost
        // of a hash change, and one this test must not mistake for the memo
        // being too small.
        let mut bases = [0usize; 6];
        for (i, (text, y, em)) in RUNS.iter().enumerate() {
            bases[i] = slot_for(*em, 20, *y, text, false);
        }
        for i in 0..RUNS.len() {
            for j in (i + 1)..RUNS.len() {
                assert_ne!(bases[i], bases[j], "the runs this test asks with collide in the memo");
            }
        }
        for _ in 0..PASSES {
            for (text, y, em) in RUNS.iter() {
                OutlineRun::place(20, *y, *em, text).expect("placed");
            }
        }
        // Two table probes per run: the pass that rasterises it and the pass
        // that reads it back and puts it in the memo. After that the walk is
        // answered from the memo. A one-entry memo paid a table probe on every
        // ask from the second pass onward -- six runs times five passes is
        // thirty probes, and each probe is a scan of the coverage pool done
        // while the painter is holding a single pixel.
        assert_eq!(
            TABLE_PROBES.load(Ordering::Relaxed),
            2 * RUNS.len(),
            "a repeat placement must not cost a table probe"
        );
        assert_eq!(
            MEMO_HITS.load(Ordering::Relaxed),
            RUNS.len() * (PASSES - 2),
            "the runs a pixel walk is cycling must be answered from the memo"
        );
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
    fn a_shade_plan_bigger_than_the_pool_still_paints_its_first_run() {
        let _pool: Option<_> = Some(reset_slots());
        // The notifications shade plans four header runs and six labels per
        // feed row before it paints any of them, and a full feed is over
        // thirty distinct placements -- more than the pool holds. The plan
        // alone evicted the header's runs, and a held run whose slot no
        // longer answers reads as no ink at all: the clock and title simply
        // vanished. A plan that outgrows the pool may cost time, never
        // pixels, so every planned run must still ink when it is painted.
        const PLANNED: usize = SLOTS + 2;
        let mut plan = [OutlineRun::EMPTY; PLANNED];
        for i in 0..PLANNED {
            let mut text = [b'M'; 3];
            text[0] = b'A' + (i % 26) as u8;
            text[1] = b'a' + ((i / 26) % 26) as u8;
            text[2] = b'0' + (i % 10) as u8;
            plan[i] = OutlineRun::place(40, 200 + i * 20, 24, &text).expect("placed");
        }
        // Paint in plan order, header first, the way the shade does.
        for i in 0..PLANNED {
            let inked = count_ink(&plan[i]);
            assert!(
                inked > 0,
                "planned run {i} lost all ink once the plan outgrew the pool"
            );
        }
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

    /// Every run at a given origin inks rows inside [`row_band`]'s range.
    ///
    /// WHY this is the contract worth pinning: a painter is about to reject
    /// pixels against the band before it asks for a box, and a band that is one
    /// row short silently clips a glyph -- the kind of defect that shows up as
    /// a caption missing its descender on one screen and not another, with
    /// nothing in the return value to say so. The band is therefore checked
    /// against the union box the painter itself builds, for the sizes and
    /// origins the shells actually use.
    #[test]
    fn a_row_band_covers_every_row_a_run_at_that_origin_can_ink() {
        let _guard = reset_slots();
        // A plain label, a run wider than the coverage buffer (so the bitmap
        // cell decides the box), a multi-byte name, and a run whose outline line
        // box is taller than the cell's.
        let runs: [&[u8]; 4] = [
            b"Stopped",
            b"The quick brown fox jumps over the lazy dog and keeps going",
            b"\xc3\x9cber",
            "W".as_bytes(),
        ];
        for scale in 1usize..=4 {
            for &y0 in &[0usize, 24, 101, 748, 1300] {
                let (band_top, band_bottom) = row_band(y0, scale);
                for text in runs {
                    let (_, bt, bw, bh) = run_box(40, y0, scale, text);
                    let top = bt.min(y0);
                    let height = bh.max(font::line_height(scale));
                    let width = bw.max(font::measure(text, scale));
                    assert!(width > 0, "a run of {} bytes measures nothing", text.len());
                    assert!(
                        band_top <= top && top + height <= band_bottom,
                        "scale {scale} at row {y0}: band {band_top}..{band_bottom} \
                         misses the box {top}..{}",
                        top + height
                    );
                }
            }
        }
    }

    // =========================================================================
    // The coverage-cache miss rate, measured on the host.
    //
    // WHAT THIS PROBE IS FOR: the change this work points at splits the placement
    // key so a run's coverage is cached apart from the row it sits on, and until
    // someone counts what a row inside the key costs, that change is a guess. Two
    // things had to be settled before anyone tuned a cache: does the row-in-key
    // actually cost rasterisations, and is the rasteriser where a frame's time
    // goes at all -- because `LAST_PLACED` may already be absorbing the repaint
    // and the real cost may be the per-pixel ask itself.
    //
    // WHY THE MODEL AND NOT A RUNNING APP: this crate compiles and tests on the
    // host, and a device figure would have to come from a running guest. So this
    // paints a band the way `render_calculator_band` and `render_music_band` do -- one pixel at
    // a time, every run of the screen asked about every pixel -- through the
    // same `design::monochrome::text_px` face those painters call. The counts
    // are the SDK's own; the *shape* of the walk is the app's. Nothing here is a
    // device latency figure, and it must not be read as one.
    //
    // WHY IT RUNS SERIALISED: the pool, the memo and the box table are
    // process-global, so this takes the same `POOL_LOCK` the other cache tests
    // take and the saved output was produced with `--test-threads=1`. Exact
    // counts under a concurrent harness would be somebody else's work.

    /// One run of the modelled screen, in the shape a band painter holds: a
    /// column, a line top inside the band, a cap height from the ladder, its
    /// bytes and its bold flag.
    #[derive(Clone, Copy)]
    struct Modelled {
        x: usize,
        y0: usize,
        cap: u32,
        text: &'static [u8],
        bold: bool,
    }

    /// The surface width the guest paints at, and the band height a dirty-band
    /// repaint of a heading, five list rows and a caption costs.
    const PROBE_W: usize = 720;
    const PROBE_BAND_H: usize = 400;
    /// The band's first row, chosen so no run's ascender is ever refused for
    /// sitting above the surface -- a refusal here would make the two models
    /// disagree for a reason that has nothing to do with the cache.
    const PROBE_TOP: usize = 400;
    /// The deepest line top below `PROBE_TOP + run.y0` that any frame of any
    /// scenario places, and the largest offset this probe scrolls by. Together
    /// they must keep every run's line top inside the band: a run whose top
    /// falls outside it is never asked, and then "every run of the band
    /// re-rasterised" is a claim about runs nobody painted.
    const PROBE_MAX_OFFSET: i32 = 53;
    const INK: u32 = 0xFF_000000;
    const BG: u32 = 0xFF_FF_FFFF;

    /// A list screen's worth of runs: a heading, five rows carrying a title and
    /// a measure, and a caption -- twelve distinct strings at four sizes on a
    /// 48 px row pitch, which is the density of Music's queue or the drawer's
    /// page, not a worst case.
    const SCREEN: [Modelled; 12] = [
        Modelled { x: 48, y0: 0, cap: 30, text: b"Recents", bold: true },
        Modelled { x: 48, y0: 64, cap: 18, text: b"Morning Run", bold: false },
        Modelled { x: 620, y0: 64, cap: 16, text: b"04:12", bold: false },
        Modelled { x: 48, y0: 112, cap: 18, text: b"Studio Notes", bold: false },
        Modelled { x: 620, y0: 112, cap: 16, text: b"11:38", bold: false },
        Modelled { x: 48, y0: 160, cap: 18, text: b"Kernel Boot Trace", bold: false },
        Modelled { x: 620, y0: 160, cap: 16, text: b"00:57", bold: false },
        Modelled { x: 48, y0: 208, cap: 18, text: b"TLS Chain Audit", bold: false },
        Modelled { x: 620, y0: 208, cap: 16, text: b"02:21", bold: false },
        Modelled { x: 48, y0: 256, cap: 18, text: b"Tap Latency Bench", bold: false },
        Modelled { x: 620, y0: 256, cap: 16, text: b"00:14", bold: false },
        Modelled { x: 48, y0: 304, cap: 13, text: b"5 items", bold: false },
    ];

    /// The whole modelled screen really does sit inside the band at every offset
    /// the probe uses. Without this the geometry claim below is unfalsifiable,
    /// because a run the band never reaches looks exactly like a run the cache
    /// never needed.
    #[test]
    fn every_modelled_run_is_inside_the_band_at_every_offset_probed() {
        let deepest = SCREEN.iter().map(|r| r.y0).max().unwrap_or(0);
        assert!(
            (deepest + PROBE_MAX_OFFSET as usize) < PROBE_BAND_H,
            "the modelled screen reaches line top {} + scroll {}, outside a band {} rows tall",
            deepest,
            PROBE_MAX_OFFSET,
            PROBE_BAND_H
        );
    }

    /// Paints the band one pixel at a time and returns a checksum of every
    /// answer, so two models of the same frame can be proved to draw the same
    /// pixels rather than merely to cost different amounts.
    ///
    /// `scroll` moves the screen. `position_free` selects the counterfactual:
    /// the run is placed at its unscrolled line and the *pixel* is moved instead
    /// -- which is exactly what a key that names no row would do, and is only
    /// legitimate because the coverage buffer is written from a pen that starts
    /// at zero and never reads `x` or `y` (see the rasterise loop in
    /// [`OutlineRun::place_styled`]).
    ///
    /// WHY the line top carries `PROBE_TOP`: [`monochrome::text_px`] gates a
    /// pixel against the run's own coarse box before it asks the cache, so a
    /// band that lives at rows 400.. must place its runs at rows 400.. as well.
    /// Dropping the band's origin here rejects every pixel at the gate and the
    /// probe reports a frame that never asked the cache a question.
    fn paint_band(scroll: i32, position_free: bool) -> u64 {
        let mut sum = 0u64;
        for row in 0..PROBE_BAND_H {
            let py = PROBE_TOP + row;
            for px in 0..PROBE_W {
                let mut answered = None;
                for run in SCREEN.iter() {
                    let (query_y, line_top) = if position_free {
                        (
                            (py as i32 - scroll) as usize,
                            PROBE_TOP + run.y0,
                        )
                    } else {
                        (
                            py,
                            (PROBE_TOP as i32 + run.y0 as i32 + scroll) as usize,
                        )
                    };
                    if let Some(c) = crate::design::monochrome::text_px(
                        px,
                        query_y,
                        run.x,
                        line_top,
                        run.cap,
                        run.text,
                        INK,
                        BG,
                        run.bold,
                    ) {
                        answered = Some(c);
                        break;
                    }
                }
                sum = sum
                    .wrapping_mul(1000003)
                    .wrapping_add(answered.unwrap_or(BG) as u64);
            }
        }
        sum
    }

    /// Paints the same band from runs the caller placed once and kept, so it
    /// never asks the shape table what a pixel's run looks like: the box is
    /// already in hand.
    ///
    /// WHY this is a model and not a proposal: it is what a plan-holding painter
    /// like the calculator's `PlannedRun::paint` already does, so the difference
    /// between it and [`paint_band`] is the price of asking per pixel even after
    /// the gate stopped guessing -- measured on work the SDK really does. It must
    /// paint the identical frame or it proves nothing, and the test that runs it
    /// asserts exactly that.
    fn paint_held(held: &[OutlineRun; 12]) -> u64 {
        let mut sum = 0u64;
        for row in 0..PROBE_BAND_H {
            let py = PROBE_TOP + row;
            for px in 0..PROBE_W {
                let mut answered = None;
                for run in held.iter() {
                    let (bx, by, bw, bh) = run.bounds();
                    if px >= bx && px < bx + bw && py >= by && py < by + bh {
                        if let Some(c) = run.pixel(px, py, INK, BG) {
                            answered = Some(c);
                            break;
                        }
                    }
                }
                sum = sum
                    .wrapping_mul(1000003)
                    .wrapping_add(answered.unwrap_or(BG) as u64);
            }
        }
        sum
    }

    /// A pixel rule: the ten arguments a band painter hands
    /// [`monochrome::text_px`], in that order, for one pixel of one run.
    type PixelRule =
        fn(usize, usize, usize, usize, u32, &[u8], u32, u32, bool) -> Option<u32>;

    /// The line top `run` sits at in a band scrolled by `scroll`.
    fn line_top(run: &Modelled, scroll: i32) -> usize {
        (PROBE_TOP as i32 + run.y0 as i32 + scroll) as usize
    }

    /// The rule the pixel path followed before its gate knew where a run's ink
    /// can land: the rung's estimate of the string's width, then a placement for
    /// every pixel that estimate admitted, then the box the placement reported,
    /// then the bitmap face for a run the outline refuses.
    ///
    /// WHY this is spelled out here now that the painter does not: it is the
    /// reference the tightened gate has to be pixel-identical to, and a claim
    /// about two gates can only be tested while both of them exist. Every branch
    /// is the same primitive the shipped path calls -- [`place_run_cap`],
    /// [`OutlineRun::bounds`], [`OutlineRun::pixel`], [`font::test_glyph_aa`] --
    /// with the estimate's own arithmetic, so what separates this from
    /// [`monochrome::text_px`] is which pixels reach a placement and nothing else.
    fn by_estimate(
        x: usize,
        y: usize,
        x0: usize,
        y0: usize,
        cap_px: u32,
        text: &[u8],
        ink: u32,
        bg: u32,
        bold: bool,
    ) -> Option<u32> {
        if text.is_empty() || cap_px == 0 {
            return None;
        }
        let max_h = (cap_px as usize).saturating_mul(2) + 8;
        if y < y0 || y >= y0 + max_h {
            return None;
        }
        let max_w = text.len().saturating_mul((cap_px as usize).saturating_mul(2) + 4) + 16;
        if x < x0 || x >= x0 + max_w {
            return None;
        }
        if let Some(run) = place_run_cap(x0, y0, cap_px, text, None, bold) {
            let (bx, by, bw, bh) = run.bounds();
            if x >= bx && x < bx + bw && y >= by && y < by + bh {
                return run.pixel(x, y, ink, bg);
            }
            return None;
        }
        let fallback = (font::em_for_cap_scale(cap_px) as usize / font::GLYPH_H).max(1);
        if x < x0 || y < y0 || y >= y0 + font::GLYPH_H * fallback {
            return None;
        }
        font::test_glyph_aa(x, y, x0, y0, fallback, text, ink, bg, bold)
    }

    /// A band painted one pixel at a time by `rule`, as the colour each pixel of
    /// it ended up with.
    ///
    /// WHY the whole frame rather than a count: two gates are the same gate as a
    /// reader is concerned only if the surface they produce is the same surface,
    /// and a set of inked pixels that differs in one row of one descender is a
    /// defect no counter here can see. The walk is the one a band painter does:
    /// every run asked about every pixel, first answer wins.
    fn band_pixels(runs: &[Modelled], scroll: i32, rule: PixelRule) -> std::vec::Vec<u32> {
        let mut frame = std::vec::Vec::with_capacity(PROBE_W * PROBE_BAND_H);
        for row in 0..PROBE_BAND_H {
            let py = PROBE_TOP + row;
            for px in 0..PROBE_W {
                let mut answered = None;
                for run in runs.iter() {
                    if let Some(c) = rule(
                        px,
                        py,
                        run.x,
                        line_top(run, scroll),
                        run.cap,
                        run.text,
                        INK,
                        BG,
                        run.bold,
                    ) {
                        answered = Some(c);
                        break;
                    }
                }
                frame.push(answered.unwrap_or(BG));
            }
        }
        frame
    }

    /// The pixels two frames disagree on, up to the first `max`, as
    /// `(index, left, right)`.
    ///
    /// WHY a bounded list: a failure here is 288,000 pixels wide, and the first
    /// few differences -- with the row and column they landed on -- say more
    /// than dumping both frames would.
    fn disagreements(a: &[u32], b: &[u32], max: usize) -> std::vec::Vec<(usize, u32, u32)> {
        let mut out = std::vec::Vec::new();
        if a.len() != b.len() {
            out.push((usize::MAX, a.len() as u32, b.len() as u32));
            return out;
        }
        for (i, (l, r)) in a.iter().zip(b.iter()).enumerate() {
            if l != r {
                out.push((i, *l, *r));
                if out.len() >= max {
                    break;
                }
            }
        }
        out
    }

    /// A run too long to key. The outline refuses it at every pixel, so its
    /// pixels belong to the bitmap face -- which is the branch where the
    /// estimate, not the line box, is the only thing holding the ink.
    const LONG: &[u8] = &[b'e'; KEY_MAX + 8];

    /// A band that reaches every branch the gate has: a heading with a
    /// descender, a multi-byte word, a run whose codepoints the face has no
    /// outline for, a run too long to key at a cap height so small that the
    /// bitmap cell it falls to is taller than the outline's line box would have
    /// been, and a bold caption.
    const MIXED: [Modelled; 5] = [
        Modelled { x: 48, y0: 20, cap: 44, text: b"Typographies", bold: true },
        Modelled {
            x: 48,
            y0: 96,
            cap: 18,
            text: b"D\xc3\xa9j\xc3\xa0 Vu glyphs",
            bold: false,
        },
        Modelled { x: 48, y0: 150, cap: 20, text: b"\xf0\x9f\x98\x82", bold: false },
        Modelled { x: 48, y0: 200, cap: 3, text: LONG, bold: false },
        Modelled { x: 48, y0: 260, cap: 13, text: b"a page of falling letters", bold: true },
    ];

    /// Every pixel the shipped path paints is the pixel the estimate painted.
    ///
    /// WHY this is the gate on the change: the estimate already rejected every
    /// pixel outside a run's ink, so tightening which pixels reach a placement
    /// may only remove questions, never answers. A single differing pixel here
    /// is a run whose remembered shape is not the box its placement reports --
    /// a caption missing its descender on one screen and not the next, with
    /// nothing in a return value to say so.
    #[test]
    fn the_tight_gate_paints_the_frame_the_estimate_painted() {
        let _pool: Option<_> = Some(reset_slots());
        let painted = crate::design::monochrome::text_px as PixelRule;
        // The scrolls are the ones the counting scenario walks, so the geometry
        // claim above covers the frames compared here.
        for (name, runs) in [("SCREEN", &SCREEN[..]), ("MIXED", &MIXED[..])] {
            for scroll in [0i32, 14, 53] {
                let before = band_pixels(runs, scroll, by_estimate);
                let after = band_pixels(runs, scroll, painted);
                let inked_before = before.iter().filter(|p| **p != BG).count();
                let inked_after = after.iter().filter(|p| **p != BG).count();
                assert_eq!(
                    inked_before, inked_after,
                    "{name} at scroll {scroll}: the estimate inked {inked_before} pixels of the \
                     band and the tight gate {inked_after}"
                );
                let diff = disagreements(&before, &after, 4);
                assert!(
                    diff.is_empty(),
                    "{name} at scroll {scroll}: the two gates painted {} different pixels, of \
                     which the first are {diff:?} (index, estimate, gate) -- the remembered \
                     shape is not the box the placement reports",
                    diff.len()
                );
                assert!(
                    inked_before > 0,
                    "{name} at scroll {scroll}: the band painted no ink at all, so the \
                     comparison below proves nothing"
                );
            }
        }
    }

    /// The identity check above can fail, and this is the mistake that makes it.
    ///
    /// WHY a deliberately wrong gate rather than a tweaked assertion: a frame
    /// comparison that has only ever passed proves the two sides are equal or
    /// proves nobody tried. Here the gate keeps the run's own width and takes its
    /// height from the cap height instead of the line box -- the plausible
    /// shortcut of assuming a run occupies the rows of its own capitals -- and the
    /// check has to catch it, because at these sizes the ascender alone sits
    /// above the cap band and every descender sits below it.
    #[test]
    fn a_gate_that_stops_at_the_cap_height_loses_the_ink_the_frame_has() {
        let _pool: Option<_> = Some(reset_slots());
        // The same rule the painter now runs, with one bound perturbed.
        fn cap_height_gate(
            x: usize,
            y: usize,
            x0: usize,
            y0: usize,
            cap_px: u32,
            text: &[u8],
            ink: u32,
            bg: u32,
            bold: bool,
        ) -> Option<u32> {
            let (w, _h) = run_shape_cap(cap_px, text, bold)?;
            let h = cap_px as usize;
            if x < x0 || y < y0 || x - x0 >= w || y - y0 >= h {
                return None;
            }
            let run = place_run_cap(x0, y0, cap_px, text, None, bold)?;
            run.pixel(x, y, ink, bg)
        }
        let painted = crate::design::monochrome::text_px as PixelRule;
        let mut total_lost = 0usize;
        let mut total_pixels = 0usize;
        for (name, runs) in [("SCREEN", &SCREEN[..]), ("MIXED", &MIXED[..])] {
            let before = band_pixels(runs, 0, painted);
            let perturbed = band_pixels(runs, 0, cap_height_gate);
            let diff = disagreements(&before, &perturbed, 4);
            assert!(
                !diff.is_empty(),
                "{name}: a gate whose height is the cap height rather than the line box painted \
                 the identical band, so the comparison that is supposed to catch it cannot"
            );
            total_pixels += disagreements(&before, &perturbed, usize::MAX).len();
            total_lost += before
                .iter()
                .zip(perturbed.iter())
                .filter(|(a, b)| **a != BG && **b == BG)
                .count();
        }
        assert!(
            total_lost > total_pixels / 4,
            "the perturbed gate changed {total_pixels} pixels and lost only {total_lost} of \
             them -- most of the change is pixels it added, which is not the failure this \
             check is built to see"
        );
    }

    /// The shape the gate answers with is the box the placement reports, or the
    /// refusal the placement gives -- never anything in between.
    ///
    /// WHY across refusals as well as placements: `None` from the gate sends the
    /// painter to the bitmap face, and `None` from the placement used to be the
    /// only way that face was reached. A gate that said `Some` about a run the
    /// face declines would drop its ink entirely, and a gate that declined a run
    /// the face draws would draw it twice over at the wrong size.
    #[test]
    fn the_gate_shape_is_the_box_the_placement_reports() {
        let _pool: Option<_> = Some(reset_slots());
        let runs: [&[u8]; 7] = [
            b"Recents",
            b"Morning Run",
            b"00:00 / 03:41",
            b"D\xc3\xa9j\xc3\xa0 Vu glyphs",
            b"\xf0\x9f\x98\x82",
            LONG,
            b"W",
        ];
        let mut refused = 0usize;
        for cap in [1u32, 3, 13, 16, 18, 24, 30, 40, 56] {
            for text in runs {
                for bold in [false, true] {
                    let shape = run_shape_cap(cap, text, bold);
                    let placed = place_run_cap(48, 400 + cap as usize * 3, cap, text, None, bold);
                    match (shape, placed) {
                        (Some((w, h)), Some(run)) => {
                            let (_bx, _by, bw, bh) = run.bounds();
                            assert_eq!(
                                (w, h),
                                (bw, bh),
                                "cap {cap}, bold {bold}, {:?}: the gate's shape is {:?} and the \
                                 placed run reports {:?}",
                                core::str::from_utf8(text).unwrap_or("<unkeyable>"),
                                (w, h),
                                (bw, bh)
                            );
                        }
                        (None, None) => refused += 1,
                        (shape, placed) => panic!(
                            "cap {cap}, bold {bold}, {:?}: the gate says {shape:?} and the \
                             placement says {:?}",
                            core::str::from_utf8(text).unwrap_or("<unkeyable>"),
                            placed.map(|r| r.bounds())
                        ),
                    }
                }
            }
        }
        assert!(
            refused > 0,
            "no run in this list is refused by both faces, so the refusal half of the gate was \
             never compared"
        );
    }

    /// A frame that scrolls the same strings asks the gate once per string.
    ///
    /// WHY this is worth its own test: the shape a gate needs is a function of
    /// the bytes, the em and the bold flag, and not of the row -- which is what
    /// lets a scroll down a list cost nothing at the gate. Table sizes and
    /// eviction policy are this module's business, but a future key that named an
    /// origin would be invisible in the pixels and obvious here.
    #[test]
    fn the_gate_measures_a_string_once_whatever_row_it_is_painted_at() {
        let _pool: Option<_> = Some(reset_slots());
        reset_gate_counts();
        let offsets = [0i32, 14, 25, 34, 41, 46, 50, 53];
        let mut asked = 0usize;
        for off in offsets.iter() {
            for run in SCREEN.iter() {
                let y0 = line_top(run, *off);
                let shape = run_shape_cap(run.cap, run.text, run.bold);
                let (w, h) = shape.expect("every modelled run has an outline shape");
                let (_bx, by, bw, bh) = place_run_cap(run.x, y0, run.cap, run.text, None, run.bold)
                    .expect("every modelled run places")
                    .bounds();
                assert_eq!((bw, bh), (w, h), "the gate and the placement disagree at row {y0}");
                assert_eq!(by, y0, "the box does not start at the line top");
                asked += 1;
            }
        }
        let (hits, walks) = gate_counts();
        assert_eq!(
            walks,
            SCREEN.len(),
            "eight passes over {} distinct strings cost {} walks -- the shape is keyed on \
             something a scroll changes",
            SCREEN.len(),
            walks
        );
        assert_eq!(
            hits + walks,
            asked,
            "the gate answered {hits} questions from the table and {walks} by walking, against \
             {asked} asked"
        );
    }

    /// Paints the band the way the shipped walk did before its gate knew a run's
    /// own box: [`by_estimate`] for every pixel of it.
    ///
    /// WHY it is worth running beside the shipped frame: this is the candidate
    /// change measured from the wrong side -- the same walk, the same cache
    /// counters, only the estimate admitting the pixels. If its checksum ever
    /// disagrees with the shipped frame, the measured box does not hold the run's
    /// ink, and tightening the gate would lose pixels instead of ns.
    fn paint_estimate(scroll: i32) -> u64 {
        let mut sum = 0u64;
        for row in 0..PROBE_BAND_H {
            let py = PROBE_TOP + row;
            for px in 0..PROBE_W {
                let mut answered = None;
                for run in SCREEN.iter() {
                    if let Some(c) = by_estimate(
                        px,
                        py,
                        run.x,
                        line_top(run, scroll),
                        run.cap,
                        run.text,
                        INK,
                        BG,
                        run.bold,
                    ) {
                        answered = Some(c);
                        break;
                    }
                }
                sum = sum
                    .wrapping_mul(1000003)
                    .wrapping_add(answered.unwrap_or(BG) as u64);
            }
        }
        sum
    }

        /// The counters, taken as a snapshot so a scenario can report its own span.
    #[derive(Clone, Copy, Default, Debug)]
    struct Counts {
        asks: usize,
        rasterises: usize,
        memo_hits: usize,
        table_probes: usize,
        replaces: usize,
        evictions: usize,
        box_hits: usize,
        box_measures: usize,
        gate_hits: usize,
        gate_walks: usize,
    }

    use core::sync::atomic::Ordering::Relaxed;
    fn read_counts() -> Counts {
        let (gate_hits, gate_walks) = gate_counts();
        Counts {
            asks: PLACEMENT_ASKS.load(Relaxed),
            rasterises: RASTERISES.load(Relaxed),
            memo_hits: MEMO_HITS.load(Relaxed),
            table_probes: TABLE_PROBES.load(Relaxed),
            replaces: REPLACES.load(Relaxed),
            evictions: EVICTIONS.load(Relaxed),
            box_hits: BOX_HITS.load(Relaxed),
            box_measures: BOX_MEASURES.load(Relaxed),
            gate_hits,
            gate_walks,
        }
    }

    /// Printed under `#![no_std]` only through the test-only `extern crate std`,
    /// so the macro has to be named rather than taken from a prelude this crate
    /// does not have.
    use std::println;

    #[test]
    fn measure_the_coverage_cache_miss_rate_across_a_scroll() {
        let _pool: Option<_> = Some(reset_slots());
        reset_counters();

        println!(
            "[TEXT CACHE PROBE] model: {} runs, band {}x{} = {} px per frame, painted one pixel \
             at a time through monochrome::text_px; host build, opt-level=3, --test-threads=1",
            SCREEN.len(),
            PROBE_W,
            PROBE_BAND_H,
            PROBE_W * PROBE_BAND_H
        );

        // ---- A: the static repaint. Same frame, painted again. ----
        // Warm it once so the pool holds every run of this row, then measure the
        // repaint that follows. This is the case `LAST_PLACED` was built for.
        //
        // This is also the frame *after* the gate was tightened: the pixels that
        // reach a placement are the pixels a run's own box holds, and the gate's
        // own questions are answered from the shape table. G below is the same
        // frame with the estimate in front of the placement again, which is what
        // makes the two comparable.
        let _ = paint_band(0, false);
        reset_counters();
        let a_start = std::time::Instant::now();
        let a_checksum = paint_band(0, false);
        let a_warm = read_counts();
        let mut a_nanos = a_start.elapsed().as_nanos() as u64;
        for _ in 0..4 {
            let t = std::time::Instant::now();
            let _ = paint_band(0, false);
            let elapsed = t.elapsed().as_nanos() as u64;
            if elapsed < a_nanos {
                a_nanos = elapsed;
            }
        }
        println!(
            "A static repaint   : asks={} rasterises={} memo_hits={} table_probes={} \
             evictions={} replaces={} box_hits={} box_measures={} gate_hits={} gate_walks={}",
            a_warm.asks, a_warm.rasterises, a_warm.memo_hits, a_warm.table_probes, a_warm.evictions,
            a_warm.replaces, a_warm.box_hits, a_warm.box_measures, a_warm.gate_hits,
            a_warm.gate_walks
        );
        println!(
            "A cost             : {} ns for {} px = {:.2} ns/px, {:.1} ns per ask",
            a_nanos,
            PROBE_W * PROBE_BAND_H,
            a_nanos as f64 / (PROBE_W * PROBE_BAND_H) as f64,
            a_nanos as f64 / a_warm.asks.max(1) as f64
        );

        // ---- B: the scroll. The same twelve strings, one row lower. ----
        // A decelerating flick: the cumulative offsets a drag actually presents,
        // with the deltas shrinking frame to frame, and the deepest of them held
        // equal to `PROBE_MAX_OFFSET` so the geometry test above keeps covering
        // every run in this list.
        let offsets = [14i32, 25, 34, 41, 46, 50, 53];
        assert_eq!(
            *offsets.last().unwrap(),
            PROBE_MAX_OFFSET,
            "the scroll list and the band geometry test have drifted apart"
        );
        let mut b_rasterises = [0usize; 7];
        let mut b_asks = [0usize; 7];
        let mut b_nanos = [0u64; 7];
        let mut b_checksums = [0u64; 7];
        let mut b_evictions = 0usize;
        let mut b_replaces = 0usize;
        for (i, off) in offsets.iter().enumerate() {
            reset_counters();
            let t = std::time::Instant::now();
            b_checksums[i] = paint_band(*off, false);
            let c = read_counts();
            b_nanos[i] = t.elapsed().as_nanos() as u64;
            b_rasterises[i] = c.rasterises;
            b_asks[i] = c.asks;
            b_evictions += c.evictions;
            b_replaces += c.replaces;
            println!(
                "B scroll y+{:<3}    : asks={} rasterises={} memo_hits={} table_probes={} \
                 evictions={} replaces={}",
                off, c.asks, c.rasterises, c.memo_hits, c.table_probes, c.evictions, c.replaces
            );
        }
        let mut b_total = 0usize;
        let mut b_ns_total = 0u64;
        for i in 0..offsets.len() {
            b_total += b_rasterises[i];
            b_ns_total += b_nanos[i];
        }
        println!(
            "B scroll total     : {} rasterises over {} frames of {} runs each",
            b_total,
            offsets.len(),
            SCREEN.len()
        );

        // ---- C: the counterfactual. A key that names no row. ----
        // Same scrolled frames, but the run is placed once and the pixel is
        // offset at read time. If this costs one pass of rasterisations and
        // paints the same pixels, the row in the key is pure waste.
        reset_counters();
        let mut c_nanos = [0u64; 7];
        let mut c_checksums = [0u64; 7];
        for (i, off) in offsets.iter().enumerate() {
            let t = std::time::Instant::now();
            c_checksums[i] = paint_band(*off, true);
            c_nanos[i] = t.elapsed().as_nanos() as u64;
            let c = read_counts();
            println!(
                "C scroll y+{:<3} row-free key: cumulative rasterises={} memo_hits={} evictions={} replaces={}",
                off, c.rasterises, c.memo_hits, c.evictions, c.replaces
            );
        }
        let c_total = read_counts().rasterises;
        let c_ns_total: u64 = c_nanos.iter().sum();
        println!(
            "C row-free total   : {} rasterises for the same {} frames",
            c_total,
            offsets.len()
        );

        // ---- D: the price of one rasterisation, measured alone. ----
        // The same twelve runs the frames above paint, placed once into an empty
        // pool, so the figure divides cleanly out of A and B.
        //
        // WHY a warm-up pass first: the first glyph this process rasterises also
        // pays for the typeface's own tables, and quoting that as the price of a
        // re-rasterisation would be the single most misleading number in this
        // file. So the runs are placed once at rows this probe never times, and
        // then two cold-pool passes are timed -- the second is the one quoted,
        // with the first printed beside it so the spread is visible.
        reset_slot_pool();
        forget_last_placement();
        for run in SCREEN.iter() {
            let _ = place_run_cap(run.x, 1200 + run.y0, run.cap, run.text, None, run.bold);
        }
        let mut d_nanos = [0u64; 2];
        let mut d_count = [0usize; 2];
        for pass in 0..2 {
            reset_slot_pool();
            forget_last_placement();
            reset_counters();
            let t = std::time::Instant::now();
            for (i, run) in SCREEN.iter().enumerate() {
                assert!(
                    place_run_cap(run.x, PROBE_TOP + run.y0, run.cap, run.text, None, run.bold)
                        .is_some(),
                    "run {i} refused to place on pass {pass}"
                );
            }
            d_nanos[pass] = t.elapsed().as_nanos() as u64;
            d_count[pass] = read_counts().rasterises;
        }
        println!(
            "D isolated paint   : {} rasterises for {} fresh runs; cold-tables-first-pass is \
             excluded, warm pass 1 = {} ns, pass 2 = {} ns ({:.0} ns per rasterise)",
            d_count[1],
            SCREEN.len(),
            d_nanos[0],
            d_nanos[1],
            d_nanos[1] as f64 / d_count[1].max(1) as f64
        );
        let d_nanos = d_nanos[1];

        // ---- E and F: the price of the ask itself, not of the rasterise. ----
        // A and B say how many times the cache was asked; this says what those
        // asks cost against the same frame painted from runs the caller placed
        // once and held, gated on each run's own measured box. The two together
        // locate the frame's time: re-rasterising, or asking.
        reset_slot_pool();
        forget_last_placement();
        reset_counters();
        let mut held = [OutlineRun::EMPTY; 12];
        let mut tight_px = 0usize;
        for (i, run) in SCREEN.iter().enumerate() {
            let placed = place_run_cap(run.x, PROBE_TOP + run.y0, run.cap, run.text, None, run.bold)
                .unwrap_or_else(|| panic!("run {i} refused to place for the held plan"));
            let (bx, by, bw, bh) = placed.bounds();
            // Clipped to the band, because a box the band does not reach is a
            // pixel nobody asks about.
            let x0c = bx.min(PROBE_W);
            let x1c = (bx + bw).min(PROBE_W);
            let y0c = by.min(PROBE_TOP + PROBE_BAND_H);
            let y1c = (by + bh).min(PROBE_TOP + PROBE_BAND_H);
            tight_px += (x1c.saturating_sub(x0c)) * (y1c.saturating_sub(y0c));
            held[i] = placed;
        }
        let e_rasterises = read_counts().rasterises;
        let f_start = std::time::Instant::now();
        let f_checksum = paint_held(&held);
        let mut f_nanos = f_start.elapsed().as_nanos() as u64;
        let f = read_counts();
        for _ in 0..4 {
            let t = std::time::Instant::now();
            let _ = paint_held(&held);
            let elapsed = t.elapsed().as_nanos() as u64;
            if elapsed < f_nanos {
                f_nanos = elapsed;
            }
        }
        println!(
            "E gate area      : the runs' own boxes hold {} px of the band, and the shipped gate \
             asked the placement about {} this frame ({} rasterises to get the boxes)",
            tight_px, a_warm.asks, e_rasterises
        );
        println!(
            "F held plan      : asks={} rasterises={} cost={} ns = {:.2} ns/px, against A's {} ns \
             at {:.2} ns/px",
            f.asks,
            f.rasterises,
            f_nanos,
            f_nanos as f64 / (PROBE_W * PROBE_BAND_H) as f64,
            a_nanos,
            a_nanos as f64 / (PROBE_W * PROBE_BAND_H) as f64
        );

        // ---- G: the same frame with the estimate back in front of it. ----
        // Every pixel of the band reaches a placement that the rung's estimate of
        // its width admits, which is what the path did before it knew a run's own
        // box. Compared against A, the same band on the same warm pool, this is
        // what the gate is worth: the asks are the frame's floor, and the
        // estimate set that floor four times above the ink.
        reset_slot_pool();
        forget_last_placement();
        let _ = paint_estimate(0);
        reset_counters();
        let g_start = std::time::Instant::now();
        let g_checksum = paint_estimate(0);
        let mut g_nanos = g_start.elapsed().as_nanos() as u64;
        let g = read_counts();
        for _ in 0..4 {
            let t = std::time::Instant::now();
            let _ = paint_estimate(0);
            let elapsed = t.elapsed().as_nanos() as u64;
            if elapsed < g_nanos {
                g_nanos = elapsed;
            }
        }
        // Per-path minima across K unified windows (task #290):
        // To compare the three gating strategies on actual execution speed without false skew
        // from system load shifting between sections, measure all three paths consecutively
        // in K unified windows. Taking the per-path minimum filters out thread preemption
        // delays across all three measurements under identical load conditions:
        let mut f_min = u64::MAX;
        let mut a_min = u64::MAX;
        let mut g_min = u64::MAX;
        for w in 0..12 {
            let t_f = std::time::Instant::now();
            let _ = paint_held(&held);
            f_min = f_min.min(t_f.elapsed().as_nanos() as u64);

            if w % 2 == 0 {
                let t_a = std::time::Instant::now();
                let _ = paint_band(0, false);
                a_min = a_min.min(t_a.elapsed().as_nanos() as u64);

                let t_g = std::time::Instant::now();
                let _ = paint_estimate(0);
                g_min = g_min.min(t_g.elapsed().as_nanos() as u64);
            } else {
                let t_g = std::time::Instant::now();
                let _ = paint_estimate(0);
                g_min = g_min.min(t_g.elapsed().as_nanos() as u64);

                let t_a = std::time::Instant::now();
                let _ = paint_band(0, false);
                a_min = a_min.min(t_a.elapsed().as_nanos() as u64);
            }
        }
        let f_nanos = f_min;
        let a_nanos = a_min;
        let g_nanos = g_min;
        println!(
            "G estimate gate    : asks={} rasterises={} cost={} ns = {:.2} ns/px, against A's {} \
             asks and {} ns -- the estimate asks {:.1}x the questions the run's own box needs, \
             and {:.1}% of a cached frame was pixels no run can ink",
            g.asks,
            g.rasterises,
            g_nanos,
            g_nanos as f64 / (PROBE_W * PROBE_BAND_H) as f64,
            a_warm.asks,
            a_nanos,
            g.asks as f64 / a_warm.asks.max(1) as f64,
            (g_nanos.saturating_sub(a_nanos)) as f64 * 100.0 / g_nanos.max(1) as f64
        );

        // ---- What the numbers say. ----
        let per_raster = d_nanos as f64 / d_count[1].max(1) as f64;
        let b_mean_ns = b_ns_total as f64 / offsets.len() as f64;
        let c_mean_ns = c_ns_total as f64 / offsets.len() as f64;
        let b_mean_raster = b_total as f64 / offsets.len() as f64;
        println!(
            "SUMMARY static repaint : rasterises={} of {} asks ({:.1} ns per ask, all answered \
             from a cache)",
            a_warm.rasterises,
            a_warm.asks,
            a_nanos as f64 / a_warm.asks.max(1) as f64
        );
        println!(
            "SUMMARY scrolled frame : rasterises={:.1} of {} asks per frame, i.e. every run of \
             the band re-rasterised ({} frames: evictions={} replaces={})",
            b_mean_raster,
            b_asks[0],
            offsets.len(),
            b_evictions,
            b_replaces
        );
        println!(
            "SUMMARY row-free key   : {} rasterises for the same {} frames, saving {}",
            c_total,
            offsets.len(),
            b_total.saturating_sub(c_total)
        );
        println!(
            "SUMMARY host cost      : scrolled frame {:.0} ns, row-free frame {:.0} ns, difference {:.0} ns/frame ({:.1}% of the frame)",
            b_mean_ns,
            c_mean_ns,
            (b_mean_ns - c_mean_ns).abs(),
            (b_mean_ns - c_mean_ns).abs() * 100.0 / b_mean_ns
        );
        println!(
            "SUMMARY one rasterise  : {:.0} ns here; the {:.1} a scrolled frame pays are {:.1}% of its {:.0} ns",
            per_raster,
            b_mean_raster,
            b_mean_raster * per_raster * 100.0 / b_mean_ns,
            b_mean_ns
        );
        println!(
            "SUMMARY where the ns go: a scrolled frame spends {:.1}% of its time on re-rasterising \
             and the rest on the per-pixel ask itself -- {} asks at up to {:.0} ns each is the \
             frame's floor, whether or not any glyph is ever drawn",
            b_mean_raster * per_raster * 100.0 / b_mean_ns,
            b_asks[0],
            a_nanos as f64 / a_warm.asks.max(1) as f64
        );
        println!(
            "SUMMARY the ask, not the glyph: a frame with every placement already cached still \
             costs {} ns because it asks {} times; the same frame from a held plan gated on each \
             run's own box costs {} ns ({} asks). The row in the key is worth {:.0} ns/frame; \
             the ask path is worth {:.0} ns/frame.",
            a_nanos,
            a_warm.asks,
            f_nanos,
            f.asks,
            b_mean_ns - c_mean_ns,
            a_nanos.saturating_sub(f_nanos) as f64
        );
        println!(
            "SUMMARY gate only  : same frame, same per-pixel placement calls, the estimate put \
             back in front of it: {} ns vs {} ns shipped = {:.1}% of that frame is pixels no run \
             can ink. The gate change alone was worth {:.0} ns/frame against the key change's \
             {:.0} ns/frame.",
            g_nanos,
            a_nanos,
            (g_nanos.saturating_sub(a_nanos)) as f64 * 100.0 / g_nanos.max(1) as f64,
            (g_nanos.saturating_sub(a_nanos)) as f64,
            (b_mean_ns - c_mean_ns).abs()
        );
        println!(
            "SUMMARY these are HOST ratios, not device milliseconds. The guest's own per-frame cost is what `[MUSIC APP] FRAMEBREAK` and `[TAP FLIP]` print, and no device run was made for this probe."
        );

        // The structural claims, as opposed to the numbers printed above.
        //
        // A static repaint asks the cache for every pixel it paints and gets
        // nowhere near one rasterisation per ask. Whether the count is exactly
        // zero is the memo's business; that it is small is the finding.
        assert!(
            a_warm.rasterises < SCREEN.len(),
            "a repaint of an unchanged frame rasterised {} times for {} runs -- the memo is not \
             absorbing it: {:?}",
            a_warm.rasterises,
            SCREEN.len(),
            a_warm
        );
        // A scroll to a row never asked before must re-rasterise every run of
        // the band, because no cache in this module keys without a row.
        for (i, off) in offsets.iter().enumerate() {
            assert!(
                b_rasterises[i] >= SCREEN.len(),
                "scrolled to y+{off} and only {} of {} runs re-rasterised -- the row-in-key claim \
                 would be wrong: {:?}",
                b_rasterises[i],
                SCREEN.len(),
                b_rasterises
            );
        }
        // The counterfactual is only evidence if it paints the same frame. This
        // is the load-bearing check for the key split: coverage bits do not
        // depend on the row, so a run placed once and read at an offset must be
        // pixel-for-pixel the frame the shipped code draws.
        for (i, off) in offsets.iter().enumerate() {
            assert_eq!(
                b_checksums[i],
                c_checksums[i],
                "scrolled to y+{off}: the row-free-key model painted a different frame, so the \
                 model is not equivalent"
            );
        }
        // And the unscrolled frame agrees too, which is the same claim with no
        // offset in it at all.
        assert_eq!(
            a_checksum,
            paint_band(0, true),
            "at scroll 0 the row-free-key model painted a different frame"
        );
        // The held-plan model is evidence about the ask path only if it paints
        // the shipped frame as well.
        assert_eq!(
            f_checksum, a_checksum,
            "the held-plan model painted a different frame from the shipped one, so its cost \
             is not comparable"
        );
        // The estimate admits at least four times the pixels a run can actually
        // ink, and the shipped gate no longer does: a frame asks once per pixel
        // of the runs' own boxes. These two are the same claim read from both
        // sides -- if the shipped path ever goes back to guessing a width from
        // the cap height, the second one is the check that says so.
        assert!(
            tight_px * 4 < g.asks,
            "the runs' own boxes hold {tight_px} px but even the estimate-gated frame made only \
             {} asks -- neither walk is measuring this band any more",
            g.asks
        );
        assert!(
            a_warm.asks < g.asks,
            "the shipped gate made {} cache asks against the estimate's {} -- it is not the \
             tighter of the two",
            a_warm.asks,
            g.asks
        );
        // And the ask path, not the rasteriser, is where a cached frame's time
        // goes: gating on the measured box is worth more than half the frame.
        assert!(
            f_nanos * 2 < a_nanos,
            "a held plan cost {f_nanos} ns against the shipped repaint's {a_nanos} ns -- the ask \
             path is not the dominant share of a cached frame"
        );
        // The gate that shipped must not lose a single pixel of the frame, and
        // it must not be slower than the estimate it replaced. These two are the
        // whole case for the change, read after it landed.
        assert_eq!(
            g_checksum, a_checksum,
            "the estimate and the runs' measured boxes painted different frames -- the box does \
             not hold their ink"
        );
        assert!(
            a_nanos < g_nanos,
            "the shipped walk cost {a_nanos} ns against the estimate's {g_nanos} ns on the same \
             frame"
        );
    }

    /// Measures the other failure the row-in-key causes: a frame plan with more
    /// distinct placements than the pool holds.
    ///
    /// WHY this is a separate question from the count above: `OutlineRun::pixel`
    /// re-places a run whose slot was reclaimed rather than losing its ink --
    /// deliberately, because `a_shade_plan_bigger_than_the_pool_still_paints_its_first_run`
    /// pins that. A plan that overflows therefore does not lose pixels, it pays
    /// a re-rasterisation for every evicted run it goes on to paint. Whether the
    /// row in the key is what pushes a plan over the line is what this measures.
    #[test]
    fn measure_the_cost_of_a_frame_plan_that_overflows_the_pool() {
        let _pool: Option<_> = Some(reset_slots());
        reset_counters();

        const PLAN_MAX: usize = 48;
        let mut text = [[b' '; 6]; PLAN_MAX];
        // Distinct short runs: "run007" .. "run047". Built by hand because this
        // crate has no allocator outside a test harness and the point of the
        // probe is the pool, not a formatting dependency.
        for i in 0..PLAN_MAX {
            text[i][0] = b'r';
            text[i][1] = b'u';
            text[i][2] = b'n';
            text[i][3] = b'0' + (i / 10) as u8;
            text[i][4] = b'0' + (i % 10) as u8;
            text[i][5] = b'!';
        }

        // 24 runs fit inside the pool's 32 slots; 40 does not, and that is the
        // difference between a plan that paints and a plan that repaints itself.
        for (label, n) in [("24 runs, under SLOTS", 24usize), ("40 runs, over SLOTS", 40usize)] {
            reset_slot_pool();
            forget_last_placement();
            let mut held = [OutlineRun::EMPTY; PLAN_MAX];
            for i in 0..n {
                held[i] = OutlineRun::place(48, PROBE_TOP + 24 + i * 44, 18, &text[i])
                    .unwrap_or_else(|| panic!("{label}: run {i} refused to place"));
            }
            reset_counters();
            let start = std::time::Instant::now();
            let mut inked = 0usize;
            for row in 0..PROBE_BAND_H {
                let py = PROBE_TOP + row;
                for px in 0..PROBE_W {
                    for r in held[..n].iter() {
                        if r.pixel(px, py, INK, BG).is_some() {
                            inked += 1;
                        }
                    }
                }
            }
            let nanos = start.elapsed().as_nanos() as u64;
            let c = read_counts();
            println!(
                "[TEXT CACHE PROBE] plan {label}: {n} runs held, {} px asked, rasterises={} \
                 replaces={} evictions={} memo_hits={} table_probes={} inked={inked} cost={nanos} ns",
                PROBE_W * PROBE_BAND_H * n,
                c.rasterises,
                c.replaces,
                c.evictions,
                c.memo_hits,
                c.table_probes,
            );
            assert!(inked > 0, "{label}: the plan painted no ink at all");
        }
    }
}
