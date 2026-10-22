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

/// The horizontal advance of `ch` at `px_per_em`, in pixels.
///
/// WHY this is the face's advance and not the measured ink: spacing is the
/// designer's statement, and measuring ink would make the gap depend on which
/// letter it happened to be.
pub fn advance_px(ch: u8, px_per_em: u32) -> u32 {
    match slot(ch) {
        Some(i) => (face::ADVANCE[i] as u32 * px_per_em) / face::UNITS_PER_EM as u32,
        None => 0,
    }
}

/// Total advance of `text` at `px_per_em`, in pixels.
pub fn text_advance_px(text: &[u8], px_per_em: u32) -> u32 {
    text.iter().map(|c| advance_px(*c, px_per_em)).sum()
}

#[inline]
fn slot(ch: u8) -> Option<usize> {
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
pub fn raster(
    ch: u8,
    px_per_em: u32,
    origin_x: i32,
    origin_y: i32,
    dst: &mut Coverage,
) -> Option<Metrics> {
    let i = slot(ch)?;
    let start = face::OUTLINE_OFFSET[i] as usize;
    let end = face::OUTLINE_OFFSET[i + 1] as usize;
    let advance = advance_px(ch, px_per_em);

    if end <= start || dst.w == 0 || dst.h == 0 || px_per_em == 0 {
        return Some(Metrics {
            advance_px: advance,
            width: 0,
            height: 0,
        });
    }

    // --- decode the outline, in font units --------------------------------
    let blob = &face::OUTLINE_BYTES[start..end];
    let mut x = [0i32; MAX_POINTS];
    let mut y = [0i32; MAX_POINTS];
    let mut on_curve = [false; MAX_POINTS];
    let mut contour_last = [0u16; MAX_CONTOURS];
    let mut n_points = 0usize;

    if blob.is_empty() {
        return None;
    }
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
        assert!(
            raster(0x00E9, 32, 32, 32, &mut cov).is_none(),
            "a char outside 32..=126"
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
}