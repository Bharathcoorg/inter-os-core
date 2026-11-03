// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Scalable outline rasteriser: the path that replaces magnifying a bitmap face.
//!
//! WHY this exists: the bitmap face is a fixed 16x24 cell drawn at an integer
//! scale, so "bigger text" means "a magnified anti-aliased edge". A magnified
//! ramp is not an edge, and at the sizes a title or a heading use it reads as
//! blur. Rasterising the real outline at the requested pixel size puts the edge
//! where the outline says it is, at any size, which is also what a
//! higher-resolution scanout will need.
//!
//! WHY no floats and no crates: this runs on bare metal in a shell whose entire
//! dependency list is `interos-std`. Positions are 16.16 fixed point and
//! coverage is integer, so the result is identical on every target and costs no
//! FPU setup. The only wider type is the `i64` used inside a scanline
//! intersection, which is why it does not affect the stored representation.
//!
//! WHY the rasteriser owns no buffer: the guest paints per pixel and cannot
//! allocate. The caller supplies the coverage surface, so an app keeps one
//! scratch buffer sized for its largest text, rasterises a glyph into it once,
//! and then answers per-pixel lookups while that glyph is drawn.

use super::face;

/// Sub-scanlines sampled per pixel row.
///
/// 16 sub-scanlines provide 17 vertical coverage levels (0..=16), eliminating
/// horizontal edge quantization and matching or exceeding 4-bit 16-level bitmap AA.
const SUBSAMPLES: i32 = 16;

/// Fixed-point one.
const ONE: i32 = 1 << 16;

/// Coverage added for a fully covered pixel, before the sub-scanline division.
///
/// 256 keeps 8 bits of headroom, so `acc / SUBSAMPLES` lands in 0..=256 and is
/// clamped to 255 only at the very end.
const FULL_COVER: i32 = 256;

/// Most points one encoded glyph may carry. TrueType allows 65535 in theory;
/// the measured peak over the encoded ASCII range of this face is 66 (`@`), so
/// a bound at double that would reject a real letter.
const MAX_POINTS: usize = 128;

/// Most contours one glyph may carry.
const MAX_CONTOURS: usize = 32;

/// Most line segments one glyph flattens into, at any render size.
///
/// Segment count comes from the outline, not from the size, so this is a
/// property of the face and not of the caller. The measured peak over the
/// encoded range is 360 (`@` at `CURVE_STEPS`), which is what the bound covers.
const MAX_SEGMENTS: usize = 512;

/// Most edge crossings on one scanline.
///
/// A scanline cannot cross more edges than the glyph has segments, so this is
/// tied to `MAX_SEGMENTS` rather than chosen: a crossing buffer that could
/// overflow would have to abort mid-fill, which would leave a partially
/// inked glyph behind.
const MAX_CROSSINGS: usize = MAX_SEGMENTS;

/// Segments a quadratic curve is flattened into.
///
/// WHY fixed rather than adaptive: an adaptive test costs more per segment than
/// the extra segments save, and at UI text sizes 8 is already below the visible
/// error of the flat run.
const CURVE_STEPS: usize = 8;

/// Most columns of ink one glyph may cover on the coverage surface.
///
/// WHY this is a glyph bound and not a surface bound: the accumulator is this
/// long, and sizing it by the surface would make a 720-wide surface demand 720
/// entries for every glyph. Ink is bounded by the em -- the widest encoded
/// glyph is `@` at 0.81 em, so this covers an em of about 630px, far past any
/// size a phone UI sets.
const MAX_ACCUM: usize = 512;

/// A coverage surface the rasteriser writes into.
///
/// `buf` is row-major with `w` pixels per row. The rasteriser never reads it and
/// never assumes anything about its contents beforehand.
pub struct Coverage<'a> {
    pub buf: &'a mut [u8],
    pub w: usize,
    pub h: usize,
}

impl<'a> Coverage<'a> {
    /// Coverage at `(x, y)`, or 0 outside the surface.
    ///
    /// WHY bounds are checked per pixel: this is called from a per-pixel
    /// painter that has usually already tested the coordinate, but a caller that
    /// gets it wrong should read 0 rather than run off the buffer.
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return 0;
        }
        self.buf[y as usize * self.w + x as usize]
    }

    fn clear_row(&mut self, y: usize, x0: usize, x1: usize) {
        let w = self.w;
        let row = &mut self.buf[y * w..y * w + w];
        for px in row.iter_mut().skip(x0).take(x1.saturating_sub(x0)) {
            *px = 0;
        }
    }

    fn put_row(&mut self, y: usize, x0: usize, acc: &[i32]) {
        let w = self.w;
        let row = &mut self.buf[y * w..y * w + w];
        for (i, px) in row.iter_mut().enumerate().take(x0 + acc.len()).skip(x0) {
            let c = (acc[i - x0] + (SUBSAMPLES / 2)) / SUBSAMPLES;
            *px = if c > 255 { 255 } else { c as u8 };
        }
    }
}

/// What a rasterised glyph occupied, so a caller can advance and align it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// Horizontal advance in pixels, already scaled.
    pub advance_px: u32,
    /// Ink width in pixels.
    pub width: usize,
    /// Ink height in pixels.
    pub height: usize,
}

/// Line metrics at `px_per_em`, as `(ascent, descent)` in pixels.
///
/// WHY this is the face's own hhea values rather than a measured bounding box:
/// line height is a property of the design, and deriving it from the ink of one
/// string would make every line a different height.
pub const fn line_metrics(px_per_em: u32) -> (u32, u32) {
    let upem = face::UNITS_PER_EM as i32;
    let ascent = (face::ASCENT as i32 * px_per_em as i32) / upem;
    let descent = (-(face::DESCENT as i32) * px_per_em as i32) / upem;
    // WHY if-then and not `max`: a const fn cannot call the const-unstable
    // `Ord::max`, and the metrics are needed in const contexts by the
    // typography styles and app layout constants.
    (
        if ascent > 0 { ascent as u32 } else { 0 },
        if descent > 0 { descent as u32 } else { 0 },
    )
}

/// The horizontal advance of codepoint `cp` at `px_per_em`, in pixels.
///
/// WHY this is the face's advance and not the measured ink: spacing is the
/// designer's statement, and measuring ink would make the gap depend on which
/// letter it happened to be.
pub fn advance_codepoint(cp: u32, px_per_em: u32) -> u32 {
    match face::lookup_glyph(cp) {
        Some((adv, _)) => (adv as u32 * px_per_em) / face::UNITS_PER_EM as u32,
        None => 0,
    }
}

/// The horizontal advance of `ch` at `px_per_em`, in pixels.
#[inline]
pub fn advance_char(ch: char, px_per_em: u32) -> u32 {
    advance_codepoint(ch as u32, px_per_em)
}

/// The horizontal advance of `ch` at `px_per_em`, in pixels.
///
/// Supports ASCII (32..=126) and 8-bit Latin-1 Supplement (160..=255).
#[inline]
pub fn advance_px(ch: u8, px_per_em: u32) -> u32 {
    advance_codepoint(ch as u32, px_per_em)
}

/// Total advance of UTF-8 `text` at `px_per_em`, in pixels.
pub fn str_advance_px(text: &str, px_per_em: u32) -> u32 {
    text.chars().map(|c| advance_codepoint(c as u32, px_per_em)).sum()
}

/// Total advance of `text` at `px_per_em`, in pixels.
///
/// Decodes valid UTF-8 and advances each Unicode codepoint; falls back
/// to byte-by-byte advances if not valid UTF-8.
///
/// WHY a memo on top of the walk: a caller that paints a screen one pixel at a
/// time asks this the same question for every pixel of the run it is inside --
/// a centred label re-derives its own width to find its left edge -- and the
/// walk is a `from_utf8` validation plus one glyph lookup per character. The
/// placement pool remembers the same run for the same reason; a width has no
/// glyphs to keep, so this one is a table of numbers.
pub fn text_advance_px(text: &[u8], px_per_em: u32) -> u32 {
    if let Some(w) = remembered_advance(text, px_per_em) {
        return w;
    }
    #[cfg(test)]
    ADVANCE_SCANS.with(|c| c.set(c.get() + 1));
    let measured = if let Ok(s) = core::str::from_utf8(text) {
        str_advance_px(s, px_per_em)
    } else {
        text.iter().map(|c| advance_px(*c, px_per_em)).sum()
    };
    remember_advance(text, px_per_em, measured);
    measured
}

/// How many widths are remembered.
///
/// WHY as many as the placement pool: the strings a screen repeats are the
/// strings it draws -- one per label, plus the same labels at the sizes the
/// ladder offers -- and a table that holds half of them answers nothing.
const ADV_SLOTS: usize = 32;

/// The longest string whose width is remembered.
///
/// WHY a bound well under the placement key's: a per-pixel caller repeats short
/// strings -- a label, a time, a digit -- and asks a long line exactly once
/// when it lays the line out. A key wide enough for prose would make the table
/// four times the size to hold the entries that are never asked twice.
const ADV_KEY_MAX: usize = 32;

#[derive(Clone, Copy)]
struct Advanced {
    em: u32,
    len: usize,
    key: [u8; ADV_KEY_MAX],
    w: u32,
}

static mut ADVANCES: [Option<Advanced>; ADV_SLOTS] = [None; ADV_SLOTS];

/// Counts the widths the table had to re-derive, for the thread that asked.
///
/// WHY a count: a cache hit and a cache miss draw the same pixels, so without
/// one there is no test that fails while the table is too small or too
/// narrow-keyed to hold what a pixel walk repeats.
///
/// WHY thread-local rather than a process global: the harness runs other tests
/// beside this one, every one of them measuring widths through the same shared
/// table, and a global count would charge their work to whichever test happened
/// to be reading. It compiles out of the guest.
#[cfg(test)]
std::thread_local! {
    static ADVANCE_SCANS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn advance_scans() -> usize {
    ADVANCE_SCANS.with(|c| c.get())
}

#[cfg(test)]
pub fn reset_advance_scans() {
    ADVANCE_SCANS.with(|c| c.set(0));
}

fn advance_slot(em: u32, text: &[u8]) -> usize {
    let first = if text.is_empty() { 0 } else { text[0] as usize };
    let last = if text.is_empty() { 0 } else { text[text.len() - 1] as usize };
    let mut h = (em as usize) ^ (text.len() << 5) ^ (first << 9) ^ (last << 3);
    h ^= (h >> 4) ^ (h >> 8);
    h % ADV_SLOTS
}

/// The remembered width of exactly (`text`, `px_per_em`), if the table holds it.
///
/// WHY the whole key is compared: a width read out for a different string would
/// put a label in the wrong place, and it is the kind of wrong that shows up on
/// one label of one screen. `len` and the bytes are checked, not a hash.
fn remembered_advance(text: &[u8], px_per_em: u32) -> Option<u32> {
    if text.len() > ADV_KEY_MAX {
        return None;
    }
    let base = advance_slot(px_per_em, text);
    unsafe {
        // SAFETY: the table is read through a raw pointer and the reference it
        // yields does not outlive this block, and nothing reached from here
        // re-enters it.
        let table = &*core::ptr::addr_of!(ADVANCES);
        for i in 0..ADV_SLOTS {
            if let Some(entry) = table[(base + i) % ADV_SLOTS].as_ref() {
                if entry.em == px_per_em
                    && entry.len == text.len()
                    && entry.key[..text.len()] == *text
                {
                    return Some(entry.w);
                }
            }
        }
    }
    None
}

/// Remembers a measured width, replacing whatever shares its slot.
///
/// WHY no least-recently-used run here, unlike the box and placement caches:
/// the cost of a wrong victim is one re-measured walk, not a re-rasterised
/// glyph, so a collision that alternating pixels cause is bounded by the walk
/// this function replaced rather than multiplied by it.
fn remember_advance(text: &[u8], px_per_em: u32, w: u32) {
    if text.len() > ADV_KEY_MAX {
        return;
    }
    let base = advance_slot(px_per_em, text);
    let mut key = [0u8; ADV_KEY_MAX];
    key[..text.len()].copy_from_slice(text);
    unsafe {
        // SAFETY: as above -- one entry written through a raw pointer, and no
        // borrow of the table is live here.
        let table = &mut *core::ptr::addr_of_mut!(ADVANCES);
        let mut target = base;
        for i in 0..ADV_SLOTS {
            let slot = (base + i) % ADV_SLOTS;
            if table[slot].is_none() {
                target = slot;
                break;
            }
        }
        table[target] = Some(Advanced {
            em: px_per_em,
            len: text.len(),
            key,
            w,
        });
    }
}

/// Forgets every remembered width.
///
/// WHY it is public: a test that reasons about the table has to start from an
/// empty one, the same reason the placement pool is reset between tests.
pub fn reset_advance_cache() {
    unsafe {
        for entry in (*core::ptr::addr_of_mut!(ADVANCES)).iter_mut() {
            *entry = None;
        }
    }
}

/// Horizontal advance with fallback to U+FFFD if the codepoint is unmapped.
pub fn advance_with_fallback(cp: u32, px_per_em: u32) -> u32 {
    let (adv, _) = face::lookup_glyph_or_fallback(cp);
    (adv as u32 * px_per_em) / face::UNITS_PER_EM as u32
}

/// Horizontal advance of the fallback glyph (`U+FFFD`) at `px_per_em`, in pixels.
pub fn fallback_advance_px(px_per_em: u32) -> u32 {
    let (adv, _) = face::fallback_glyph();
    (adv as u32 * px_per_em) / face::UNITS_PER_EM as u32
}

/// Fallback replacement character codepoint (`U+FFFD`).
pub const REPLACEMENT_CHAR: u32 = face::REPLACEMENT_CODEPOINT;

/// Fallback glyph replacement: returns `cp` if supported by the typeface,
/// or `U+FFFD` if unmapped.
pub fn fallback_glyph_replacement(cp: u32) -> u32 {
    if face::lookup_glyph(cp).is_some() {
        cp
    } else {
        REPLACEMENT_CHAR
    }
}

#[inline]
pub fn slot(ch: u8) -> Option<usize> {
    if (face::FIRST_CHAR..=face::LAST_CHAR).contains(&ch) {
        Some((ch - face::FIRST_CHAR) as usize)
    } else {
        None
    }
}

#[inline]
fn be_i16(data: &[u8], at: usize) -> i16 {
    (((data[at] as u16) << 8) | data[at + 1] as u16) as i16
}

/// Rasterises `ch` at `px_per_em` into `dst`.
///
/// Supports ASCII (32..=126) and Latin-1 8-bit (160..=255).
#[inline]
pub fn raster(
    ch: u8,
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    raster_codepoint(ch as u32, px_per_em, origin_x, origin_y, dst)
}

/// Rasterises Unicode codepoint `cp` at `px_per_em` into `dst`.
///
/// `origin_x`/`origin_y` place the pen on the baseline inside `dst`: `x` grows
/// right, `y` grows DOWN, so a glyph's ink above the baseline lands at negative
/// `y` relative to the pen. Passing the position explicitly rather than assuming
/// it is `(0, 0)` is what lets a caller keep one scratch buffer and put the
/// baseline wherever that glyph needs it -- including below the buffer's first
/// row, which is the normal case for an ascender.
///
/// Returns `None` when the character has no outline at all, when it is outside
/// the encoded range, or when the glyph is more complex than the fixed bounds
/// above allow. A `None` is a refusal rather than a failure: nothing is written
/// and the caller keeps whatever it had.
pub fn raster_codepoint(
    cp: u32,
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    let (adv, blob) = face::lookup_glyph(cp)?;
    let advance = (adv as u32 * px_per_em) / face::UNITS_PER_EM as u32;
    raster_outline(blob, advance, px_per_em, origin_x, origin_y, dst)
}

/// Rasterises Unicode character `ch` at `px_per_em` into `dst`.
#[inline]
pub fn raster_char(
    ch: char,
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    raster_codepoint(ch as u32, px_per_em, origin_x, origin_y, dst)
}

/// Rasterises Unicode codepoint `cp` into `dst`, replacing unmapped codepoints
/// with the `U+FFFD` fallback replacement glyph.
pub fn raster_with_fallback(
    cp: u32,
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    let (adv, blob) = face::lookup_glyph_or_fallback(cp);
    let advance = (adv as u32 * px_per_em) / face::UNITS_PER_EM as u32;
    raster_outline(blob, advance, px_per_em, origin_x, origin_y, dst)
}

/// Rasterises the `U+FFFD` fallback replacement glyph into `dst`.
pub fn raster_fallback(
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    let (adv, blob) = face::fallback_glyph();
    let advance = (adv as u32 * px_per_em) / face::UNITS_PER_EM as u32;
    raster_outline(blob, advance, px_per_em, origin_x, origin_y, dst)
}

/// Internal helper: decodes and rasterises a glyph outline blob into `dst`.
fn raster_outline(
    blob: &[u8],
    advance: u32,
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    if blob.is_empty() || dst.w == 0 || dst.h == 0 || px_per_em == 0 {
        return Some(Metrics {
            advance_px: advance,
            width: 0,
            height: 0,
        });
    }

    // --- decode the outline, in font units --------------------------------
    let mut x = [0i32; MAX_POINTS];
    let mut y = [0i32; MAX_POINTS];
    let mut on_curve = [false; MAX_POINTS];
    let mut contour_last = [0u16; MAX_CONTOURS];
    let mut n_points = 0usize;

    let n_contours = blob[0] as usize;
    if n_contours > MAX_CONTOURS {
        return None;
    }
    let mut at = 1usize;
    for c in 0..n_contours {
        if at >= blob.len() {
            return None;
        }
        let count = blob[at] as usize;
        at += 1;
        if n_points + count > MAX_POINTS {
            return None;
        }
        for _ in 0..count {
            if at + 5 > blob.len() {
                return None;
            }
            on_curve[n_points] = blob[at] != 0;
            x[n_points] = be_i16(blob, at + 1) as i32;
            y[n_points] = be_i16(blob, at + 3) as i32;
            n_points += 1;
            at += 5;
        }
        contour_last[c] = (n_points - 1) as u16;
    }
    if n_points == 0 {
        // A glyph with no contours -- a space, or one the face left blank --
        // still advances the pen, so it is a successful raster with no ink.
        // A contour that declared points but delivered none is a corrupt
        // table, and that is refused.
        if n_contours == 0 {
            return Some(Metrics {
                advance_px: advance,
                width: 0,
                height: 0,
            });
        }
        return None;
    }

    // --- scale into screen space ------------------------------------------
    //
    // `scale` maps one font unit to `scale` 16.16 pixels, so a coordinate is
    // scaled by MULTIPLYING and stays in 16.16. Dividing by `ONE` here instead
    // would collapse the point onto the origin, because the pen offset added
    // below is itself 16.16.
    //
    // TrueType's y grows up and pixels grow down, so y is negated here, once,
    // and everything after this is in screen space.
    let upem = face::UNITS_PER_EM as i64;
    let scale = (px_per_em as i64 * ONE as i64 + upem / 2) / upem;
    // One font unit of headroom: the widest a coordinate can be is an i16, so
    // any size past that is refused rather than wrapped.
    if i16::MAX as i64 * scale > (1 << 30) {
        return None;
    }
    for p in 0..n_points {
        x[p] = (x[p] as i64 * scale) as i32 + origin_x * ONE;
        y[p] = -(y[p] as i64 * scale) as i32 + origin_y * ONE;
    }

    // --- flatten to line segments ----------------------------------------
    let mut sx0 = [0i32; MAX_SEGMENTS];
    let mut sy0 = [0i32; MAX_SEGMENTS];
    let mut sx1 = [0i32; MAX_SEGMENTS];
    let mut sy1 = [0i32; MAX_SEGMENTS];
    let mut n_seg = 0usize;

    let mut first = 0usize;
    for c in 0..n_contours {
        let last = contour_last[c] as usize;
        if last < first {
            first = last + 1;
            continue;
        }
        if last == first {
            // A lone point draws nothing; TrueType permits it in empty contours.
            first = last + 1;
            continue;
        }
        n_seg = flatten_contour(
            &x,
            &y,
            &on_curve,
            first,
            last,
            &mut sx0,
            &mut sy0,
            &mut sx1,
            &mut sy1,
            n_seg,
        )?;
        first = last + 1;
    }
    if n_seg == 0 {
        return Some(Metrics {
            advance_px: advance,
            width: 0,
            height: 0,
        });
    }

    // --- bounding box, clipped to the surface -----------------------------
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for s in 0..n_seg {
        min_x = min_x.min(sx0[s]).min(sx1[s]);
        max_x = max_x.max(sx0[s]).max(sx1[s]);
        min_y = min_y.min(sy0[s]).min(sy1[s]);
        max_y = max_y.max(sy0[s]).max(sy1[s]);
    }

    let row_lo = min_y >> 16;
    let row_hi = (max_y >> 16) + 1;
    // Clipped to the surface in usize space, so every later index is already
    // known to be in range and the scanline loop needs no further checks.
    let col_lo = (min_x >> 16).max(0) as usize;
    let col_hi = (((max_x >> 16) + 1).max(0) as usize).min(dst.w);
    let row_lo_us = (row_lo.max(0) as usize).min(dst.h);
    let row_hi_us = (row_hi.max(0) as usize).min(dst.h);

    if col_lo >= col_hi || row_lo_us >= row_hi_us {
        // Entirely off this surface: report the extent anyway so a caller can
        // still align, but draw nothing.
        return Some(Metrics {
            advance_px: advance,
            width: col_hi.saturating_sub(col_lo),
            height: (row_hi - row_lo).max(0) as usize,
        });
    }

    // --- scanline fill ----------------------------------------------------
    //
    // The accumulator is indexed from the glyph's first inked column, not from
    // the surface's left edge, so the buffer this loop needs is the size of the
    // glyph's ink rather than the size of the screen. That is what lets one
    // scratch buffer serve a 720-wide surface: a glyph's ink is a fraction of
    // an em, and the em is the only thing the caller chooses.
    let span = col_hi - col_lo;
    let base = col_lo as i32 * ONE;
    let mut acc = [0i32; MAX_ACCUM];
    if span > acc.len() {
        // Ink wider than the bound -- a glyph at an em far past any UI size --
        // is refused rather than clipped, and the caller keeps its fallback.
        return None;
    }
    let mut cross_x = [0i32; MAX_CROSSINGS];
    let mut cross_dir = [0i8; MAX_CROSSINGS];

    for row in row_lo_us..row_hi_us {
        dst.clear_row(row, col_lo, col_hi);
        // The accumulator is per row: it sums the sub-scanlines of THIS row
        // only. Leaving the previous row's sum in place would darken every row
        // below the first one that was covered.
        for v in acc.iter_mut().take(span) {
            *v = 0;
        }
        for s in 0..SUBSAMPLES {
            // Sample line through this row, in 16.16 centered at (s + 0.5) / SUBSAMPLES.
            let sy = (row as i32) * ONE + ((s * 2 + 1) * ONE) / (2 * SUBSAMPLES);
            let mut n_cross = 0usize;
            for k in 0..n_seg {
                let (x0, y0, x1, y1) = (sx0[k], sy0[k], sx1[k], sy1[k]);
                // Half-open test: a vertex shared by two segments is counted
                // once, which is what stops a join from being double-covered.
                if (y0 <= sy) == (y1 <= sy) {
                    continue;
                }
                let num = ((sy - y0) as i64) * ((x1 - x0) as i64);
                let den = (y1 - y0) as i64;
                let hit = x0 as i64 + num / den;
                cross_x[n_cross] = hit as i32 - base;
                cross_dir[n_cross] = if y1 > y0 { 1 } else { -1 };
                n_cross += 1;
            }
            if n_cross < 2 {
                continue;
            }
            // Insertion sort: a scanline crosses a handful of edges, and this
            // is stable and allocation-free.
            for a in 1..n_cross {
                let mut b = a;
                while b > 0 && cross_x[b - 1] > cross_x[b] {
                    cross_x.swap(b - 1, b);
                    cross_dir.swap(b - 1, b);
                    b -= 1;
                }
            }
            // Non-zero winding: a span is inside while the winding is non-zero.
            let mut winding = 0i32;
            let mut span_start = 0i32;
            for k in 0..n_cross {
                let prev = winding;
                winding += cross_dir[k] as i32;
                if prev == 0 && winding != 0 {
                    span_start = cross_x[k];
                } else if prev != 0 && winding == 0 {
                    add_span(&mut acc, span_start, cross_x[k]);
                }
            }
        }
        dst.put_row(row, col_lo, &acc[..span]);
    }

    Some(Metrics {
        advance_px: advance,
        width: col_hi - col_lo,
        height: row_hi_us - row_lo_us,
    })
}

/// Adds the horizontal span `[xa, xb]` (16.16 fixed point) to the accumulator,
/// giving the end pixels their fractional share.
///
/// WHY fractional ends rather than rounding to whole pixels: at a 1px stem width
/// a rounded end is the difference between a stem and a gap.
fn add_span(acc: &mut [i32], xa: i32, xb: i32) {
    if xb <= xa {
        return;
    }
    let pa = xa >> 16;
    let pb = xb >> 16;
    let put = |acc: &mut [i32], idx: i32, v: i32| {
        if idx >= 0 && (idx as usize) < acc.len() {
            acc[idx as usize] += v;
        }
    };
    if pa == pb {
        put(acc, pa, (xb - xa) >> 8);
        return;
    }
    put(acc, pa, (ONE - (xa & 0xFFFF)) >> 8);
    for px in (pa + 1)..pb {
        put(acc, px, FULL_COVER);
    }
    put(acc, pb, (xb & 0xFFFF) >> 8);
}

/// Flattens one contour into line segments, appending them at `n_seg`.
///
/// Returns `None` when the contour would exceed `MAX_SEGMENTS`; the caller then
/// refuses the glyph rather than drawing a truncated letter.
fn flatten_contour(
    x: &[i32; MAX_POINTS],
    y: &[i32; MAX_POINTS],
    on_curve: &[bool; MAX_POINTS],
    first: usize,
    last: usize,
    sx0: &mut [i32; MAX_SEGMENTS],
    sy0: &mut [i32; MAX_SEGMENTS],
    sx1: &mut [i32; MAX_SEGMENTS],
    sy1: &mut [i32; MAX_SEGMENTS],
    mut n_seg: usize,
) -> Option<usize> {
    let count = last - first + 1;
    if count >= MAX_POINTS || count + 2 > MAX_POINTS {
        return None;
    }

    // Start from an on-curve point. A contour whose first point is a control
    // point has an implied start: the midpoint between its last and first
    // points, and iteration begins there so the walk always starts on the
    // outline rather than on a curve's control point.
    let mut on_curve_start = None;
    for i in 0..count {
        if on_curve[first + i] {
            on_curve_start = Some(first + i);
            break;
        }
    }
    // Collect the contour's points, rotated so iteration starts at that point.
    // When there is no on-curve point at all the contour is all controls; the
    // implied midpoint is emitted as the first point below.
    let mut px = [0i32; MAX_POINTS];
    let mut py = [0i32; MAX_POINTS];
    let mut pon = [false; MAX_POINTS];
    let mut m = 0usize;
    if on_curve_start.is_none() {
        if m >= MAX_POINTS {
            return None;
        }
        px[m] = (x[first] + x[last]) / 2;
        py[m] = (y[first] + y[last]) / 2;
        pon[m] = true;
        m += 1;
    }
    let begin_at = on_curve_start.unwrap_or(first);
    for i in 0..count {
        if m >= MAX_POINTS {
            return None;
        }
        let p = begin_at + i;
        if p > last {
            // Wrap: only needed because `begin_at` may be inside the contour.
            let q = first + (p - (last + 1));
            px[m] = x[q];
            py[m] = y[q];
            pon[m] = on_curve[q];
        } else {
            px[m] = x[p];
            py[m] = y[p];
            pon[m] = on_curve[p];
        }
        m += 1;
    }
    // Repeat the first point to close the contour.
    if m >= MAX_POINTS {
        return None;
    }
    px[m] = px[0];
    py[m] = py[0];
    pon[m] = pon[0];
    m += 1;

    let mut cx = px[0];
    let mut cy = py[0];
    let mut i = 1usize;
    while i < m {
        if pon[i] {
            if n_seg == MAX_SEGMENTS {
                return None;
            }
            sx0[n_seg] = cx;
            sy0[n_seg] = cy;
            sx1[n_seg] = px[i];
            sy1[n_seg] = py[i];
            n_seg += 1;
            cx = px[i];
            cy = py[i];
            i += 1;
        } else {
            // Quadratic: control point here, end point next (implied midpoint
            // when two control points follow each other).
            let (ex, ey) = if i + 1 < m && pon[i + 1] {
                (px[i + 1], py[i + 1])
            } else {
                ((px[i] + px[i + 1]) / 2, (py[i] + py[i + 1]) / 2)
            };
            for step in 1..=CURVE_STEPS {
                if n_seg == MAX_SEGMENTS {
                    return None;
                }
                let t = step as i64 * ONE as i64 / CURVE_STEPS as i64;
                let inv = ONE as i64 - t;
                // Quadratic Bezier at t, evaluated in 64-bit and shifted back.
                let nx = ((inv * inv * cx as i64
                    + 2 * inv * t * px[i] as i64
                    + t * t * ex as i64)
                    / (ONE as i64 * ONE as i64)) as i32;
                let ny = ((inv * inv * cy as i64
                    + 2 * inv * t * py[i] as i64
                    + t * t * ey as i64)
                    / (ONE as i64 * ONE as i64)) as i32;
                sx0[n_seg] = cx;
                sy0[n_seg] = cy;
                sx1[n_seg] = nx;
                sy1[n_seg] = ny;
                n_seg += 1;
                cx = nx;
                cy = ny;
            }
            i += 1;
        }
    }
    Some(n_seg)
}

// ---------------------------------------------------------------------------
// Text Direction & BiDi Detection
// ---------------------------------------------------------------------------

/// Text layout direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    /// Left-to-right (Latin, Cyrillic, Greek, etc.).
    Ltr,
    /// Right-to-left (Arabic, Hebrew, Syriac, Thaana, etc.).
    Rtl,
}

/// Returns true if codepoint `cp` is a right-to-left character (Arabic, Hebrew, Syriac, Thaana, etc.).
pub fn is_rtl_codepoint(cp: u32) -> bool {
    matches!(
        cp,
        0x0590..=0x05FF // Hebrew
        | 0xFB1D..=0xFB4F // Hebrew Presentation Forms
        | 0x0600..=0x06FF // Arabic
        | 0x0750..=0x077F // Arabic Supplement
        | 0x0870..=0x089F // Arabic Extended-B
        | 0x08A0..=0x08FF // Arabic Extended-A
        | 0xFB50..=0xFDFF // Arabic Presentation Forms-A
        | 0xFE70..=0xFEFF // Arabic Presentation Forms-B
        | 0x0700..=0x074F // Syriac
        | 0x0780..=0x07BF // Thaana
        | 0x0800..=0x083F // Samaritan
        | 0x0840..=0x085F // Mandaic
        | 0x200F          // Right-to-Left Mark (RLM)
        | 0x202B          // Right-to-Left Embedding (RLE)
        | 0x202E          // Right-to-Left Override (RLO)
        | 0x2067          // Right-to-Left Isolate (RLI)
    )
}

/// Returns true if character `ch` is a right-to-left character (Arabic, Hebrew, etc.).
#[inline]
pub fn is_rtl_char(ch: char) -> bool {
    is_rtl_codepoint(ch as u32)
}

/// Returns true if codepoint `cp` is a strong left-to-right character.
pub fn is_strong_ltr_codepoint(cp: u32) -> bool {
    matches!(
        cp,
        0x0041..=0x005A // A-Z
        | 0x0061..=0x007A // a-z
        | 0x00C0..=0x00D6 // Latin-1 Supplement letters
        | 0x00D8..=0x00F6
        | 0x00F8..=0x00FF
        | 0x0100..=0x024F // Latin Extended-A and B
        | 0x0370..=0x03FF // Greek
        | 0x0400..=0x04FF // Cyrillic
        | 0x3000..=0x9FFF // CJK symbols, kana, ideographs
        | 0xAC00..=0xD7AF // Hangul
    )
}

/// Returns true if character `ch` is a strong left-to-right character.
#[inline]
pub fn is_strong_ltr_char(ch: char) -> bool {
    is_strong_ltr_codepoint(ch as u32)
}

/// Returns true if `text` has right-to-left directionality.
///
/// Follows the standard Unicode Bidirectional heuristic (UAX #9 rule P2):
/// inspects characters in order and resolves direction by the first strong
/// directional character (Arabic, Hebrew, Syriac, Thaana vs Latin, Greek, CJK).
/// If no strong directional character is present, defaults to false (LTR).
pub fn is_rtl_run(text: &str) -> bool {
    for ch in text.chars() {
        if is_rtl_char(ch) {
            return true;
        } else if is_strong_ltr_char(ch) {
            return false;
        }
    }
    false
}

/// Returns true if byte run `bytes` has right-to-left directionality.
/// Decodes as UTF-8 if valid; otherwise inspects byte values.
pub fn is_rtl_run_bytes(bytes: &[u8]) -> bool {
    if let Ok(s) = core::str::from_utf8(bytes) {
        is_rtl_run(s)
    } else {
        bytes.iter().any(|&b| is_rtl_codepoint(b as u32))
    }
}

/// Computes the horizontal pen alignment offset for a line of text.
///
/// For LTR text, pen starts at `0` (left edge).
/// For RTL text, pen aligns to the right edge: `container_w.saturating_sub(text_w)`.
#[inline]
pub fn rtl_align_offset(text_w: usize, container_w: usize, is_rtl: bool) -> usize {
    if is_rtl && container_w > text_w {
        container_w - text_w
    } else {
        0
    }
}

/// Layout hook for line positioning: determines starting pen X coordinate
/// inside a container `[container_x, container_x + container_w]` based on
/// text direction.
#[inline]
pub fn layout_line_x(container_x: usize, container_w: usize, text_w: usize, is_rtl: bool) -> usize {
    container_x + rtl_align_offset(text_w, container_w, is_rtl)
}

/// High-level line layout hook that measures text advance, detects BiDi direction,
/// and computes the starting pen `x` position within `[container_x, container_x + container_w]`.
///
/// Returns `(pen_x, text_w, direction)`.
pub fn layout_line_hook(
    text: &str,
    px_per_em: u32,
    container_x: usize,
    container_w: usize,
) -> (usize, usize, TextDirection) {
    let is_rtl = is_rtl_run(text);
    let dir = if is_rtl {
        TextDirection::Rtl
    } else {
        TextDirection::Ltr
    };
    let text_w = str_advance_px(text, px_per_em) as usize;
    let pen_x = layout_line_x(container_x, container_w, text_w, is_rtl);
    (pen_x, text_w, dir)
}

/// High-level line layout hook for byte slices (UTF-8 or raw bytes).
pub fn layout_line_bytes_hook(
    text: &[u8],
    px_per_em: u32,
    container_x: usize,
    container_w: usize,
) -> (usize, usize, TextDirection) {
    let is_rtl = is_rtl_run_bytes(text);
    let dir = if is_rtl {
        TextDirection::Rtl
    } else {
        TextDirection::Ltr
    };
    let text_w = text_advance_px(text, px_per_em) as usize;
    let pen_x = layout_line_x(container_x, container_w, text_w, is_rtl);
    (pen_x, text_w, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Largest surface a test renders into. Sized for a 48px em with a 2px margin
    /// on each side, which is past the largest size any test asks for.
    const TEST_SURFACE: usize = 128;

    /// Rasterises `ch` at `px_per_em` onto a generous surface and returns it.
    ///
    /// WHY a fixed array and not `Vec`: these tests run under the crate's
    /// `no_std` configuration, and the point of the module is that it never
    /// allocates -- a test that allocated would not be testing that.
    fn render(ch: u8, px_per_em: u32) -> ([u8; TEST_SURFACE * TEST_SURFACE], usize, Metrics) {
        let w = (px_per_em as usize * 2).min(TEST_SURFACE);
        let h = (px_per_em as usize * 2).min(TEST_SURFACE);
        let mut buf = [0u8; TEST_SURFACE * TEST_SURFACE];
        let mut cov = Coverage {
            buf: &mut buf[..w * h],
            w,
            h,
        };
        // The baseline is placed where a caller would put it: inside the buffer,
        // one em down, so both the ascender and the descender have room.
        let origin_x = px_per_em as i32;
        let origin_y = px_per_em as i32;
        let m = raster(ch, px_per_em, origin_x, origin_y, &mut cov).expect("glyph rasterised");
        (buf, w, m)
    }

    #[test]
    fn a_letter_puts_ink_on_the_surface() {
        let (buf, _w, m) = render(b'A', 32);
        let ink: usize = buf.iter().filter(|c| **c > 0).count();
        assert!(ink > 20, "expected a visible letter, got {ink} inked pixels");
        assert!(m.width > 4 && m.height > 8, "implausible extent {m:?}");
        assert!(m.advance_px > 0);
    }

    #[test]
    fn coverage_is_anti_aliased_rather_than_binary() {
        let (buf, _w, _m) = render(b'S', 32);
        let mid: usize = buf
            .iter()
            .filter(|c| **c > 40 && **c < 215)
            .count();
        assert!(
            mid > 20,
            "expected anti-aliased edge pixels along a curve, got {mid}"
        );
    }

    #[test]
    fn a_glyph_drawn_larger_has_more_ink_pixels() {
        // WHY this matters: it is the property the bitmap face cannot provide.
        // Magnifying a 16x24 cell scales its pixels and stays at two colours
        // along an edge, so ink pixels grow quadratically with area; a real
        // outline adds pixels at every size.
        let (small, _, ms) = render(b'A', 24);
        let (large, _, ml) = render(b'A', 48);
        let ink_small = small.iter().filter(|c| **c > 0).count() as f64;
        let ink_large = large.iter().filter(|c| **c > 0).count() as f64;
        assert!(ink_large > ink_small * 1.8, "ink {ink_small} -> {ink_large}");
        assert!(ml.width > ms.width && ml.height > ms.height);
    }

    #[test]
    fn a_space_advances_without_putting_ink() {
        let (buf, _w, m) = render(b' ', 32);
        assert_eq!(buf.iter().filter(|c| **c > 0).count(), 0);
        assert!(m.advance_px > 0, "a space still advances the pen");
        assert_eq!(m.width, 0);
    }

    #[test]
    fn a_character_outside_the_encoded_range_is_refused() {
        let mut buf = [0u8; 64 * 64];
        let mut cov = Coverage {
            buf: &mut buf,
            w: 64,
            h: 64,
        };
        // Unmapped control character should be refused
        assert!(
            raster(0x01, 32, 32, 32, &mut cov).is_none(),
            "a control char outside encoded tables"
        );
        // Unmapped high codepoint should be refused by raster_codepoint
        assert!(
            raster_codepoint(0x1F600, 32, 32, 32, &mut cov).is_none(),
            "an unmapped emoji codepoint"
        );
    }

    #[test]
    fn advances_scale_with_the_requested_size() {
        let small = advance_px(b'M', 16);
        let large = advance_px(b'M', 32);
        assert_eq!(large, small * 2);
        assert_eq!(text_advance_px(b"MM", 16), small * 2);
    }

    #[test]
    fn line_metrics_follow_the_face_rather_than_the_ink() {
        // Roboto Condensed's hhea ascender is 2146 of 2048 em, so at 48px the
        // ascent is a little over the em. The bound is the face's own value, so
        // a regression in the metric shows up as a wrong number, not as a pass
        // from a bound nobody derived.
        let em = 48u32;
        let (ascent, descent) = line_metrics(em);
        let want_ascent = face::ASCENT as u32 * em / face::UNITS_PER_EM as u32;
        let want_descent = (-face::DESCENT as u32) * em / face::UNITS_PER_EM as u32;
        assert_eq!(ascent, want_ascent, "ascent is the face's hhea value");
        assert_eq!(descent, want_descent, "descent is the face's hhea value");
        assert!(ascent > em, "an ascender is taller than the em");
        assert!(descent > 0);
    }

    #[test]
    fn ink_lands_inside_the_reported_extent() {
        let (buf, w, m) = render(b'g', 40);
        let inked = buf.iter().filter(|c| **c > 0).count();
        assert!(inked > 0);
        assert!(
            m.width <= w && m.width > 0,
            "extent {m:?} does not fit a {w}-wide surface"
        );
    }

    #[test]
    fn flatten_contour_rejects_oversized_contour_safely() {
        let x = [0i32; MAX_POINTS];
        let y = [0i32; MAX_POINTS];
        let on_curve = [true; MAX_POINTS];
        let mut sx0 = [0i32; MAX_SEGMENTS];
        let mut sy0 = [0i32; MAX_SEGMENTS];
        let mut sx1 = [0i32; MAX_SEGMENTS];
        let mut sy1 = [0i32; MAX_SEGMENTS];
        let res = flatten_contour(&x, &y, &on_curve, 0, 127, &mut sx0, &mut sy0, &mut sx1, &mut sy1, 0);
        assert!(res.is_none());
    }

    #[test]
    fn latin1_accented_characters_render() {
        // Test common accented characters: é, ñ, ç, ß, À, °
        for &ch in &[0x00E9u32, 0x00F1, 0x00E7, 0x00DF, 0x00C0, 0x00B0] {
            let mut buf = [0u8; 64 * 64];
            let mut cov = Coverage {
                buf: &mut buf,
                w: 64,
                h: 64,
            };
            let m = raster_codepoint(ch, 32, 16, 40, &mut cov).expect("rasterises glyph");
            let ink = buf.iter().filter(|c| **c > 0).count();
            assert!(ink > 10, "expected ink for codepoint 0x{ch:04X}, got {ink}");
            assert!(m.advance_px > 0);
            assert!(m.width > 0 && m.height > 0);
        }
    }

    #[test]
    fn latin_extended_a_characters_render() {
        // Test Latin Extended-A characters: Ā, č, š, ž
        for &ch in &[0x0100u32, 0x010D, 0x0161, 0x017E] {
            let mut buf = [0u8; 64 * 64];
            let mut cov = Coverage {
                buf: &mut buf,
                w: 64,
                h: 64,
            };
            let m = raster_codepoint(ch, 32, 16, 40, &mut cov).expect("rasterises glyph");
            let ink = buf.iter().filter(|c| **c > 0).count();
            assert!(ink > 10, "expected ink for codepoint 0x{ch:04X}, got {ink}");
            assert!(m.advance_px > 0);
        }
    }

    #[test]
    fn typographical_symbols_render() {
        // Em-dash, en-dash, bullet, curly quotes, ellipsis, euro
        for &ch in &[0x2014u32, 0x2013, 0x2022, 0x201C, 0x201D, 0x2026, 0x20AC] {
            let mut buf = [0u8; 64 * 64];
            let mut cov = Coverage {
                buf: &mut buf,
                w: 64,
                h: 64,
            };
            let m = raster_codepoint(ch, 32, 16, 40, &mut cov).expect("rasterises glyph");
            let ink = buf.iter().filter(|c| **c > 0).count();
            assert!(ink > 5, "expected ink for symbol 0x{ch:04X}, got {ink}");
            assert!(m.advance_px > 0);
        }
    }

    #[test]
    fn fallback_glyph_replacement_renders_and_advances() {
        // Unmapped codepoints
        let unmapped = 0x1F600u32; // Grinning face emoji
        assert_eq!(fallback_glyph_replacement(unmapped), REPLACEMENT_CHAR);
        assert_eq!(fallback_glyph_replacement(0x0041), 0x0041); // 'A' is mapped

        // Raster with fallback
        let mut buf = [0u8; 64 * 64];
        let mut cov = Coverage {
            buf: &mut buf,
            w: 64,
            h: 64,
        };
        let m = raster_with_fallback(unmapped, 32, 16, 40, &mut cov).expect("renders fallback");
        let ink = buf.iter().filter(|c| **c > 0).count();
        assert!(ink > 15, "expected ink for fallback glyph, got {ink}");
        assert_eq!(m.advance_px, fallback_advance_px(32));
        assert!(m.advance_px > 0);
    }

    #[test]
    fn bidi_detection_and_layout_hook() {
        // RTL characters: Hebrew and Arabic
        assert!(is_rtl_char('\u{05E9}')); // Hebrew Shin
        assert!(is_rtl_char('\u{0645}')); // Arabic Meem
        assert!(is_rtl_codepoint(0x05D0)); // Hebrew Alef
        assert!(is_rtl_codepoint(0x0627)); // Arabic Alef

        // Non-RTL characters
        assert!(!is_rtl_char('A'));
        assert!(!is_rtl_char('é'));
        assert!(!is_rtl_char('5'));
        assert!(!is_rtl_char(' '));
        assert!(!is_rtl_codepoint(0x0020));

        // RTL runs
        assert!(is_rtl_run("שלום"));
        assert!(is_rtl_run("مرحبا"));
        assert!(is_rtl_run("  שלום world")); // Starts with Hebrew after neutral space
        assert!(!is_rtl_run("Hello שלום")); // Starts with Latin
        assert!(!is_rtl_run("12345"));
        assert!(is_rtl_run_bytes("שלום".as_bytes()));
        assert!(!is_rtl_run_bytes(b"Hello world"));

        // Layout line hook
        let container_x = 10;
        let container_w = 200;
        let (pen_ltr, w_ltr, dir_ltr) = layout_line_hook("Hello", 24, container_x, container_w);
        assert_eq!(dir_ltr, TextDirection::Ltr);
        assert_eq!(pen_ltr, container_x); // Left-aligned
        assert!(w_ltr > 0);

        let (pen_rtl, w_rtl, dir_rtl) = layout_line_hook("שלום", 24, container_x, container_w);
        assert_eq!(dir_rtl, TextDirection::Rtl);
        assert_eq!(pen_rtl, container_x + container_w - w_rtl); // Right-aligned
        assert!(pen_rtl > container_x);
    }

    #[test]
    fn text_advance_px_supports_utf8_and_latin1() {
        let ascii_adv = text_advance_px(b"Cafe", 24);
        let utf8_adv = text_advance_px("Café".as_bytes(), 24);
        assert!(utf8_adv > 0);
        assert!(ascii_adv > 0);
        assert_eq!(str_advance_px("Café", 24), utf8_adv);

        // Degree symbol and bullets
        let deg_adv = text_advance_px("24°C".as_bytes(), 24);
        assert!(deg_adv > text_advance_px(b"24C", 24));

        let bullet_adv = text_advance_px("• item".as_bytes(), 24);
        assert!(bullet_adv > text_advance_px(b"item", 24));
    }

    #[test]
    fn a_repeated_width_is_measured_once_and_not_once_per_pixel() {
        use std::println;
        reset_advance_cache();
        reset_advance_scans();
        const ASKS: usize = 20_000;
        // A centred label derives its own width for every pixel it paints, so
        // the walk -- one UTF-8 validation plus one glyph lookup per character
        // -- is what a per-pixel caller repeats. The table has to answer it.
        // The bound is a ratio rather than an exact count because the harness
        // runs other tests through this same shared table beside this one, and
        // one of them can take the entry; what the test is asserting is that
        // the walk is not what the repeat costs.
        for _ in 0..ASKS {
            assert_eq!(text_advance_px(b"09:20", 56), str_advance_px("09:20", 56));
        }
        let scans = advance_scans();
        println!("advance scans: {} of {} asks", scans, ASKS);
        assert!(
            scans * 100 <= ASKS,
            "{scans} of {ASKS} repeats re-walked the face; the width memo answered nothing"
        );
    }

    #[test]
    fn the_remembered_width_is_the_walks_own_answer() {
        // The cache may only ever return what the walk would have. Every case
        // is measured cold, then warm, then against the uncached walk directly,
        // so a wrong key match, a truncation, or a stale entry shows up as a
        // label landing in the wrong place rather than as a rendering that
        // merely looks plausible.
        reset_advance_cache();
        for text in [
            b"09:20".as_slice(),
            b"Mon, 12 Oct".as_slice(),
            "Café".as_bytes(),
            "• item".as_bytes(),
            b"Ag".as_slice(),
            b"Ai".as_slice(),
            b"".as_slice(),
            "a long line that the width memo deliberately refuses to hold because it is past the key".as_bytes(),
        ] {
            for em in [17u32, 24, 34, 56, 102] {
                reset_advance_cache();
                let cold = text_advance_px(text, em);
                let warm = text_advance_px(text, em);
                let again = text_advance_px(text, em);
                let walk = match core::str::from_utf8(text) {
                    Ok(s) => str_advance_px(s, em),
                    Err(_) => text.iter().map(|c| advance_px(*c, em)).sum(),
                };
                assert_eq!(cold, walk, "the first answer at em {em} is not the walk's");
                assert_eq!(warm, walk, "the remembered answer at em {em} is not the walk's");
                assert_eq!(again, walk, "the second repeat at em {em} is not the walk's");
            }
        }
        // Two same-length strings asked in alternation: the shorter key's entry
        // must not answer for the longer one's, which is the shape a truncated
        // compare would get wrong.
        reset_advance_cache();
        let mut a = 0u32;
        let mut b = 0u32;
        for i in 0..64 {
            if i % 2 == 0 {
                a = text_advance_px(b"Ag", 40);
            } else {
                b = text_advance_px(b"Ai", 40);
            }
        }
        assert_eq!(a, str_advance_px("Ag", 40));
        assert_eq!(b, str_advance_px("Ai", 40));
    }
}