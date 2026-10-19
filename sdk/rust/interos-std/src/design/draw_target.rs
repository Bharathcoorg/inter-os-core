//! A `DrawTarget` for the guest's own ARGB8888 surfaces.
//!
//! WHY this exists: an app that wants a rounded rectangle writes a per-pixel
//! colour function and a chunk loop today, which is a great deal of code for one
//! card, and it is the shape every guest app repeats. The Rust embedded
//! ecosystem already solved that, and this is the adapter that lets a guest app
//! draw instead of hand-rolling a painter.
//!
//! WHY `embedded-graphics-core` and not the full `embedded-graphics`: core is the
//! display-driver boundary specifically -- its own documentation says it is for
//! "crates that extend embedded-graphics... By implementing a draw target for a
//! display driver, all embedded-graphics drawables can be drawn to that
//! display." This module is a display driver. It carries no primitives, so it is
//! the smaller dependency for the same interface.
//!
//! WHY this is a measured decision, not a preference. Before writing it I ran the
//! experiment: implement the target, draw five shapes through it, and count.
//! `draw_iter` received **zero pixels in every case**. Filled rects, filled
//! circles, grids of cards and a whole-surface clear all route through
//! `fill_solid`, which hands the target a row at a time -- exactly the span path
//! the guest's own band work is built on. The dependency's marginal cost is
//! 27,648 bytes, about 0.5% of the guest image, which is less than one app's
//! padding.
//!
//! WHAT THIS DOES NOT SUPPLY, stated up front so nobody discovers it later: a
//! texture store, a widget toolkit, layout, dirty regions, or animation. It is a
//! drawing interface. The thing it replaces is the per-pixel painter.

pub use embedded_graphics_core::draw_target::DrawTarget;
pub use embedded_graphics_core::geometry::{Dimensions, OriginDimensions, Point, Size};
pub use embedded_graphics_core::primitives::Rectangle;
pub use embedded_graphics_core::pixelcolor::PixelColor;
pub use embedded_graphics_core::Pixel;

/// The colour type the guest's surfaces carry.
///
/// WHY a newtype rather than reusing `Rgb888`: the surface is ARGB8888 and a
/// primitive's colour arrives as RGB, so someone has to supply the alpha. Doing
/// it here, once, means no caller can route a translucent colour into a surface
/// that has no slot for one.
///
/// WHY the alpha is forced to opaque rather than blended: the guest's compositor
/// already composites surfaces against each other with Porter-Duff alpha, so
/// per-surface alpha is the layer that job belongs to. A primitive that carried
/// translucency would be a second compositor living inside the app, and the two
/// would disagree about what is behind what.
///
/// WHY `Raw` is `()`: the crate's own `RawData` implementation for `()` documents
/// it as usable "if raw data conversion isn't required", which is exactly this
/// case -- the colour is packed for the surface directly and never needs the
/// packed-integer form the ecosystem's crates use for their own storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceColor(pub u32);

impl SurfaceColor {
    pub const TRANSPARENT: SurfaceColor = SurfaceColor(0x0000_0000);
    pub const OPAQUE: SurfaceColor = SurfaceColor(0xFF00_0000);

    /// Packs a colour the way a guest surface carries it, ARGB8888.
    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        SurfaceColor(0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    /// Packs an explicit ARGB value, for a caller that already has alpha.
    pub const fn argb(argb: u32) -> Self {
        SurfaceColor(argb)
    }

    pub const fn to_argb(self) -> u32 {
        self.0
    }
}

impl PixelColor for SurfaceColor {
    type Raw = ();
}

/// A linear ARGB8888 buffer a guest app draws into.
///
/// This is the adapter itself. It owns no geometry and no compositor; it maps a
/// drawing call onto the caller's buffer the same way the guest's own surface
/// writes map onto theirs.
pub struct Surface<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl<'a> Surface<'a> {
    /// Wraps a buffer of at least `width * height` pixels.
    ///
    /// WHY the size is checked here rather than trusted: a caller that hands over
    /// a short buffer would get silent truncation, and a surface that truncates
    /// is a screen with a stripe down it.
    pub fn new(
        pixels: &'a mut [u32],
        width: usize,
        height: usize,
    ) -> Result<Self, &'static str> {
        if width == 0 || height == 0 {
            return Err("a surface cannot be zero-sized");
        }
        let total_pixels = width.checked_mul(height).ok_or("surface dimensions overflow usize")?;
        if pixels.len() < total_pixels {
            return Err("buffer is smaller than the surface it is asked to be");
        }
        Ok(Self {
            pixels,
            width,
            height,
        })
    }

    /// Returns the width of the surface in pixels.
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Returns the height of the surface in pixels.
    pub const fn height(&self) -> usize {
        self.height
    }

    /// The packed pixel already at a surface coordinate.
    pub fn get(&self, x: usize, y: usize) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.pixels[y * self.width + x])
    }

    /// Sets the packed pixel at a surface coordinate.
    /// Returns `true` if the coordinate was in bounds, `false` otherwise.
    pub fn set(&mut self, x: usize, y: usize, color: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.pixels[y * self.width + x] = color;
        true
    }

    /// Creates a Surface targeting the canonical native physical scanout (1080x2160).
    pub fn new_scanout(pixels: &'a mut [u32]) -> Result<Self, &'static str> {
        Self::new(
            pixels,
            super::viewport::SCANOUT_WIDTH,
            super::viewport::SCANOUT_HEIGHT,
        )
    }

    /// Draws an anti-aliased rounded rectangle with optional 1px/multi-px border.
    pub fn draw_aa_rounded_rect(
        &mut self,
        rect_x: usize,
        rect_y: usize,
        rect_w: usize,
        rect_h: usize,
        radius_px: usize,
        fill_argb: u32,
        border_argb: Option<u32>,
        border_width_px: usize,
    ) {
        draw_aa_rounded_rect_px(
            self.pixels,
            self.width,
            self.width,
            self.height,
            rect_x,
            rect_y,
            rect_w,
            rect_h,
            radius_px,
            fill_argb,
            border_argb,
            border_width_px,
        );
    }

    /// Returns the underlying pixel buffer as an immutable slice.
    pub fn pixels(&self) -> &[u32] {
        self.pixels
    }

    /// Returns the underlying pixel buffer as a mutable slice.
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        self.pixels
    }
}

/// Blends foreground ARGB color over background ARGB color using 8-bit coverage (0..=255).
#[inline]
pub fn blend_coverage_argb(fg: u32, bg: u32, coverage: u8) -> u32 {
    if coverage == 0 {
        return bg;
    }
    if coverage == 255 && (fg >> 24) == 0xFF {
        return fg;
    }
    let a_fg = (fg >> 24) & 0xFF;
    let r_fg = (fg >> 16) & 0xFF;
    let g_fg = (fg >> 8) & 0xFF;
    let b_fg = fg & 0xFF;

    let a_bg = (bg >> 24) & 0xFF;
    let r_bg = (bg >> 16) & 0xFF;
    let g_bg = (bg >> 8) & 0xFF;
    let b_bg = bg & 0xFF;

    let eff_a = (a_fg * coverage as u32 + 127) / 255;
    let inv_a = 255 - eff_a;

    let out_r = (r_fg * eff_a + r_bg * inv_a + 127) / 255;
    let out_g = (g_fg * eff_a + g_bg * inv_a + 127) / 255;
    let out_b = (b_fg * eff_a + b_bg * inv_a + 127) / 255;
    let out_a = (a_fg * eff_a + a_bg * inv_a + 127) / 255;

    (out_a << 24) | (out_r << 16) | (out_g << 8) | out_b
}

/// Renders a rectangle with 8-bit coverage-antialiased rounded corners and optional 1px border.
pub fn draw_aa_rounded_rect_px(
    target: &mut [u32],
    stride: usize,
    target_w: usize,
    target_h: usize,
    rect_x: usize,
    rect_y: usize,
    rect_w: usize,
    rect_h: usize,
    radius_px: usize,
    fill_argb: u32,
    border_argb: Option<u32>,
    border_width_px: usize,
) {
    if rect_w == 0 || rect_h == 0 || stride == 0 {
        return;
    }
    let r = radius_px.min(rect_w / 2).min(rect_h / 2);
    let r_i32 = r as i32;
    let y_max = (rect_y + rect_h).min(target_h);
    let x_max = (rect_x + rect_w).min(target_w);
    let border_w_i32 = border_width_px as i32;

    for y in rect_y..y_max {
        let row_idx = y * stride;
        let in_top = r > 0 && y < rect_y + r;
        let in_bottom = r > 0 && y >= rect_y + rect_h - r;
        let cy = if in_top {
            (rect_y + r) as i32
        } else if in_bottom {
            (rect_y + rect_h - r - 1) as i32
        } else {
            0
        };

        for x in rect_x..x_max {
            let idx = row_idx + x;
            if idx >= target.len() {
                continue;
            }
            let in_left = r > 0 && x < rect_x + r;
            let in_right = r > 0 && x >= rect_x + rect_w - r;

            if (in_left || in_right) && (in_top || in_bottom) {
                let cx = if in_left {
                    (rect_x + r) as i32
                } else {
                    (rect_x + rect_w - r - 1) as i32
                };
                let dx = (x as i32) - cx;
                let dy = (y as i32) - cy;
                let dist_sq = dx * dx + dy * dy;

                if dist_sq > (r_i32 + 1) * (r_i32 + 1) {
                    continue;
                }

                // Sub-pixel 8-bit coverage calculation along corner arc
                let coverage = if dist_sq <= (r_i32.saturating_sub(1)) * (r_i32.saturating_sub(1)) {
                    255u8
                } else {
                    let dist_approx = isqrt_approx(dist_sq);
                    let edge_lo = (r_i32 * 256) - 128;
                    let edge_hi = (r_i32 * 256) + 128;
                    if dist_approx <= edge_lo {
                        255u8
                    } else if dist_approx >= edge_hi {
                        0u8
                    } else {
                        let cov = 255 - ((dist_approx - edge_lo) * 255 / (edge_hi - edge_lo));
                        cov.clamp(0, 255) as u8
                    }
                };

                if coverage == 0 {
                    continue;
                }

                if let Some(b_color) = border_argb {
                    if border_width_px > 0 {
                        let inner_r = r_i32.saturating_sub(border_w_i32);
                        if dist_sq >= inner_r * inner_r {
                            target[idx] = blend_coverage_argb(b_color, target[idx], coverage);
                            continue;
                        }
                    }
                }
                target[idx] = blend_coverage_argb(fill_argb, target[idx], coverage);
                continue;
            }

            // Straight edges
            if let Some(b_color) = border_argb {
                if border_width_px > 0
                    && (x < rect_x + border_width_px
                        || x >= rect_x + rect_w - border_width_px
                        || y < rect_y + border_width_px
                        || y >= rect_y + rect_h - border_width_px)
                {
                    target[idx] = blend_coverage_argb(b_color, target[idx], 255);
                    continue;
                }
            }
            target[idx] = blend_coverage_argb(fill_argb, target[idx], 255);
        }
    }
}

#[inline]
fn isqrt_approx(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let n_shifted = (n as u64) << 16;
    let mut root = 0u64;
    let mut bit = 1u64 << 46;
    while bit > n_shifted {
        bit >>= 2;
    }
    let mut remainder = n_shifted;
    while bit != 0 {
        if remainder >= root + bit {
            remainder -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root as i32
}

// `Dimensions` comes from the blanket impl over `OriginDimensions` below, so it
// is deliberately not written here -- a second impl of the same trait for the
// same type is a conflict, not a convenience.
impl<'a> OriginDimensions for Surface<'a> {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl<'a> DrawTarget for Surface<'a> {
    type Color = SurfaceColor;
    type Error = core::convert::Infallible;

    /// Draws a pixel iterator.
    ///
    /// WHY this is not where the work happens: the experiment showed the
    /// primitives that matter call `fill_solid` instead, so this path is for
    /// genuinely scattered pixels -- a glyph mask, a point plot -- rather than
    /// for anything a shape would produce.
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = embedded_graphics_core::Pixel<Self::Color>>,
    {
        for pixel in pixels {
            let point = pixel.0;
            if point.x < 0 || point.y < 0 {
                continue;
            }
            let x = point.x as usize;
            let y = point.y as usize;
            if x >= self.width || y >= self.height {
                continue;
            }
            self.pixels[y * self.width + x] = pixel.1.to_argb();
        }
        Ok(())
    }

    /// Fills a run of contiguous pixels, one row at a time.
    ///
    /// WHY this is overridden rather than falling through to `draw_iter`: the
    /// measurement is that this is what a filled primitive actually calls, and a
    /// row is the contiguous span the guest's surface writes are made of. Turning
    /// a card into a pixel-at-a-time iterator would be the slow path the band work
    /// exists to avoid.
    fn fill_contiguous<I>(
        &mut self,
        rectangle: &Rectangle,
        colors: I,
    ) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        let rw = rectangle.size.width as u64;
        let rh = rectangle.size.height as u64;
        if rw == 0 || rh == 0 {
            return Ok(());
        }
        let rx = rectangle.top_left.x as i64;
        let ry = rectangle.top_left.y as i64;
        let w = self.width as i64;
        let h = self.height as i64;

        // If the rectangle is entirely outside the canvas bounds on any side,
        // no pixels can ever land on the surface. Return early.
        if ry >= h || rx >= w || ry.saturating_add(rh as i64) <= 0 || rx.saturating_add(rw as i64) <= 0 {
            return Ok(());
        }

        let mut iter = colors.into_iter();
        for dy in 0..rh as i64 {
            let y = ry.saturating_add(dy);
            if y >= h {
                // All subsequent rows are strictly beyond the bottom edge.
                return Ok(());
            }
            for dx in 0..rw as i64 {
                let Some(color) = iter.next() else { return Ok(()); };
                let x = rx.saturating_add(dx);
                if x >= 0 && x < w && y >= 0 {
                    self.pixels[y as usize * self.width + x as usize] = color.to_argb();
                }
            }
        }
        Ok(())
    }

    /// Fills a whole rectangle.
    fn fill_solid(&mut self, rectangle: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let w = self.width as i64;
        let h = self.height as i64;

        let rx = rectangle.top_left.x as i64;
        let ry = rectangle.top_left.y as i64;
        let rw = rectangle.size.width as i64;
        let rh = rectangle.size.height as i64;

        if rw <= 0 || rh <= 0 {
            return Ok(());
        }

        let x0 = rx.clamp(0, w) as usize;
        let y0 = ry.clamp(0, h) as usize;
        let x1 = rx.saturating_add(rw).clamp(0, w) as usize;
        let y1 = ry.saturating_add(rh).clamp(0, h) as usize;

        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        for y in y0..y1 {
            let row = y * self.width;
            self.pixels[row + x0..row + x1].fill(color.to_argb());
        }
        Ok(())
    }

    /// Clears the whole surface.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        if let Some(len) = self.width.checked_mul(self.height) {
            if len <= self.pixels.len() {
                self.pixels[..len].fill(color.to_argb());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: SurfaceColor = SurfaceColor(0xFFFF_FFFF);

    #[test]
    fn a_surface_refuses_a_buffer_smaller_than_its_stated_size() {
        // WHY this is the first test: a caller that hands over a short buffer
        // would get silent truncation, and a surface that truncates is a screen
        // with a stripe down it.
        let mut buf = [0u32; 100];
        assert!(Surface::new(&mut buf, 10, 11).is_err());
        assert!(Surface::new(&mut [0u32; 0], 10, 10).is_err());
        assert!(Surface::new(&mut [0u32; 0], 0, 10).is_err());
    }

    #[test]
    fn a_fill_solid_writes_only_its_own_rows() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        let rect = Rectangle::new(Point::new(10, 10), Size::new(20, 5));
        surface.fill_solid(&rect, WHITE).unwrap();
        assert_eq!(buf[10 * 100 + 20], WHITE.to_argb(), "a row it covers");
        assert_eq!(buf[10 * 100 + 9], 0, "the row before");
        assert_eq!(buf[15 * 100 + 10], 0, "the row after");
        let inked = buf.iter().filter(|p| **p == WHITE.to_argb()).count();
        assert_eq!(inked, 100, "a 20x5 rect is 100 pixels");
    }

    #[test]
    fn a_rect_past_the_right_edge_is_clipped_not_written() {
        // WHY clipping: a caller drawing past its own surface must lose the
        // pixels it does not own, not write over the buffer beyond them.
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        let rect = Rectangle::new(Point::new(90, 10), Size::new(20, 5));
        surface.fill_solid(&rect, WHITE).unwrap();
        assert_eq!(surface.get(90, 10), Some(WHITE.to_argb()));
        assert_eq!(surface.get(99, 10), Some(WHITE.to_argb()));
        let inked = buf.iter().filter(|p| **p == WHITE.to_argb()).count();
        assert_eq!(inked, 50, "the 10 columns that exist");
    }

    #[test]
    fn a_rect_past_negative_bounds_is_clipped_safely() {
        {
            let mut buf = [0u32; 100 * 100];
            let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
            // Entirely off left edge: x=-50, w=20
            let rect_off = Rectangle::new(Point::new(-50, 10), Size::new(20, 5));
            surface.fill_solid(&rect_off, WHITE).unwrap();
            assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
        }

        {
            let mut buf = [0u32; 100 * 100];
            let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
            // Straddling left edge: x=-10, w=20 (so x in [0, 10) is visible)
            let rect_straddle = Rectangle::new(Point::new(-10, 10), Size::new(20, 5));
            surface.fill_solid(&rect_straddle, WHITE).unwrap();
            let inked = buf.iter().filter(|p| **p == WHITE.to_argb()).count();
            assert_eq!(inked, 50, "10 visible columns * 5 rows");
        }
    }

    #[test]
    fn a_rect_entirely_off_surface_writes_nothing() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        let rect = Rectangle::new(Point::new(200, 10), Size::new(20, 5));
        surface.fill_solid(&rect, WHITE).unwrap();
        assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
    }

    #[test]
    fn fill_contiguous_writes_row_major_and_stops_when_colors_run_out() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        let rect = Rectangle::new(Point::new(0, 0), Size::new(3, 3));
        surface.fill_contiguous(&rect, [WHITE, WHITE]).unwrap();
        assert_eq!(buf[0], WHITE.to_argb());
        assert_eq!(buf[1], WHITE.to_argb());
        assert_eq!(buf[2], 0, "third pixel in row 0 untouched");
        assert_eq!(buf[100], 0, "row 1 untouched");
    }

    #[test]
    fn draw_iter_drops_pixels_outside_the_surface() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        surface
            .draw_iter([
                embedded_graphics_core::Pixel(Point::new(0, 0), WHITE),
                embedded_graphics_core::Pixel(Point::new(-1, 0), WHITE),
                embedded_graphics_core::Pixel(Point::new(0, -1), WHITE),
                embedded_graphics_core::Pixel(Point::new(100, 0), WHITE),
                embedded_graphics_core::Pixel(Point::new(0, 100), WHITE),
            ])
            .unwrap();
        assert_eq!(buf[0], WHITE.to_argb());
        assert_eq!(buf.iter().filter(|p| **p == WHITE.to_argb()).count(), 1);
    }

    #[test]
    fn a_clear_fills_the_whole_surface() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        surface.clear(WHITE).unwrap();
        assert!(buf.iter().all(|p| *p == WHITE.to_argb()));
    }

    #[test]
    fn the_bounding_box_matches_the_surface() {
        let mut buf = [0u32; 100 * 100];
        let surface = Surface::new(&mut buf, 100, 100).expect("surface");
        assert_eq!(surface.bounding_box().size, Size::new(100, 100));
        assert_eq!(surface.bounding_box().top_left, Point::zero());
    }

    #[test]
    fn a_color_packs_as_argb8888_the_surface_expects() {
        let c = SurfaceColor::from_rgb(0x12, 0x34, 0x56);
        assert_eq!(c.to_argb(), 0xFF12_3456);
        assert_eq!(SurfaceColor::argb(0x8012_3456).to_argb(), 0x8012_3456);
        assert_eq!(SurfaceColor::TRANSPARENT.to_argb(), 0);
    }

    #[test]
    fn fill_contiguous_advances_iterator_correctly_across_negative_boundaries() {
        // 3x3 rectangle starting at (-1, -1) on a 10x10 surface.
        // The iterator supplies 9 distinct colors:
        // C0(-1,-1) C1(0,-1) C2(1,-1)
        // C3(-1, 0) C4(0, 0) C5(1, 0)
        // C6(-1, 1) C7(0, 1) C8(1, 1)
        // Visible on surface:
        // (0,0) -> C4, (1,0) -> C5
        // (0,1) -> C7, (1,1) -> C8
        let mut buf = [0u32; 10 * 10];
        let mut surface = Surface::new(&mut buf, 10, 10).expect("surface");
        let colors = [
            SurfaceColor(0xFF00_0001), SurfaceColor(0xFF00_0002), SurfaceColor(0xFF00_0003),
            SurfaceColor(0xFF00_0004), SurfaceColor(0xFF00_0005), SurfaceColor(0xFF00_0006),
            SurfaceColor(0xFF00_0007), SurfaceColor(0xFF00_0008), SurfaceColor(0xFF00_0009),
        ];
        let rect = Rectangle::new(Point::new(-1, -1), Size::new(3, 3));
        surface.fill_contiguous(&rect, colors).unwrap();

        assert_eq!(surface.get(0, 0), Some(0xFF00_0005)); // C4 at index 4
        assert_eq!(surface.get(1, 0), Some(0xFF00_0006)); // C5 at index 5
        assert_eq!(surface.get(0, 1), Some(0xFF00_0008)); // C7 at index 7
        assert_eq!(surface.get(1, 1), Some(0xFF00_0009)); // C8 at index 8

        // Verify that no other pixels were modified
        let written_count = buf.iter().filter(|p| **p != 0).count();
        assert_eq!(written_count, 4);
    }

    #[test]
    fn fill_solid_and_contiguous_handle_extreme_overflow_gracefully() {
        // 1. Extreme positive coordinates outside surface bounds
        {
            let mut buf = [0u32; 10 * 10];
            let mut surface = Surface::new(&mut buf, 10, 10).expect("surface");
            let rect_overflow = Rectangle::new(Point::new(i32::MAX - 2, i32::MAX - 2), Size::new(u32::MAX, u32::MAX));
            assert!(surface.fill_solid(&rect_overflow, WHITE).is_ok());
            assert!(surface.fill_contiguous(&rect_overflow, core::iter::empty()).is_ok());
            drop(surface);
            assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
        }

        // 2. Extreme negative coordinates completely off-screen
        {
            let mut buf = [0u32; 10 * 10];
            let mut surface = Surface::new(&mut buf, 10, 10).expect("surface");
            let rect_neg_offscreen = Rectangle::new(Point::new(i32::MIN + 2, i32::MIN + 2), Size::new(1000, 1000));
            assert!(surface.fill_solid(&rect_neg_offscreen, WHITE).is_ok());
            assert!(surface.fill_contiguous(&rect_neg_offscreen, core::iter::empty()).is_ok());
            drop(surface);
            assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
        }

        // 3. Huge straddling rectangle across negative and positive bounds
        {
            let mut buf = [0u32; 10 * 10];
            let mut surface = Surface::new(&mut buf, 10, 10).expect("surface");
            let rect_huge = Rectangle::new(Point::new(-1000, -1000), Size::new(u32::MAX, u32::MAX));
            assert!(surface.fill_solid(&rect_huge, WHITE).is_ok());
            drop(surface);
            assert_eq!(buf.iter().filter(|p| **p == WHITE.to_argb()).count(), 100);
        }
    }

    #[test]
    fn surface_new_rejects_multiplication_overflow() {
        let mut buf = [0u32; 16];
        // usize::MAX / 2 * 4 overflows usize
        let overflow_w = usize::MAX / 2;
        let overflow_h = 4;
        assert!(Surface::new(&mut buf, overflow_w, overflow_h).is_err());
    }

    #[test]
    fn fill_contiguous_multicolor_multi_row_with_clipping_all_boundaries() {
        // 4x4 multicolor grid:
        // C00 C01 C02 C03
        // C10 C11 C12 C13
        // C20 C21 C22 C23
        // C30 C31 C32 C33
        // Placed at Point::new(-1, -1) on a 2x2 surface.
        // Surface visible pixels:
        // (0,0) -> C11
        // (1,0) -> C12
        // (0,1) -> C21
        // (1,1) -> C22
        let colors = [
            SurfaceColor(0x10), SurfaceColor(0x11), SurfaceColor(0x12), SurfaceColor(0x13),
            SurfaceColor(0x20), SurfaceColor(0x21), SurfaceColor(0x22), SurfaceColor(0x23),
            SurfaceColor(0x30), SurfaceColor(0x31), SurfaceColor(0x32), SurfaceColor(0x33),
            SurfaceColor(0x40), SurfaceColor(0x41), SurfaceColor(0x42), SurfaceColor(0x43),
        ];

        let mut buf = [0u32; 2 * 2];
        let mut surface = Surface::new(&mut buf, 2, 2).expect("surface");
        let rect = Rectangle::new(Point::new(-1, -1), Size::new(4, 4));
        surface.fill_contiguous(&rect, colors).unwrap();

        assert_eq!(surface.get(0, 0), Some(0x21));
        assert_eq!(surface.get(1, 0), Some(0x22));
        assert_eq!(surface.get(0, 1), Some(0x31));
        assert_eq!(surface.get(1, 1), Some(0x32));
    }

    #[test]
    fn fill_contiguous_offscreen_terminates_immediately_without_consuming_infinite_iter() {
        let mut buf = [0u32; 10 * 10];
        let mut surface = Surface::new(&mut buf, 10, 10).expect("surface");

        // Below the screen: Point(0, 50), Size(100, 100_000)
        let rect_below = Rectangle::new(Point::new(0, 50), Size::new(100, 100_000));
        let infinite_colors = core::iter::repeat(WHITE);
        assert!(surface.fill_contiguous(&rect_below, infinite_colors).is_ok());

        // Above the screen: Point(0, -200), Size(100, 50) -> ry + rh = -150 <= 0
        let rect_above = Rectangle::new(Point::new(0, -200), Size::new(100, 50));
        let infinite_colors_above = core::iter::repeat(WHITE);
        assert!(surface.fill_contiguous(&rect_above, infinite_colors_above).is_ok());

        // To the right: Point(50, 0), Size(100, 100) -> rx >= w
        let rect_right = Rectangle::new(Point::new(50, 0), Size::new(100, 100));
        let infinite_colors_right = core::iter::repeat(WHITE);
        assert!(surface.fill_contiguous(&rect_right, infinite_colors_right).is_ok());

        // To the left: Point(-200, 0), Size(50, 100) -> rx + rw <= 0
        let rect_left = Rectangle::new(Point::new(-200, 0), Size::new(50, 100));
        let infinite_colors_left = core::iter::repeat(WHITE);
        assert!(surface.fill_contiguous(&rect_left, infinite_colors_left).is_ok());

        // Buffer remains completely untouched
        assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
    }

    #[test]
    fn fill_solid_zero_dimensions_is_safe_noop() {
        let mut buf = [0u32; 10 * 10];
        let mut surface = Surface::new(&mut buf, 10, 10).expect("surface");
        let rect_zero_w = Rectangle::new(Point::new(0, 0), Size::new(0, 10));
        let rect_zero_h = Rectangle::new(Point::new(0, 0), Size::new(10, 0));
        assert!(surface.fill_solid(&rect_zero_w, WHITE).is_ok());
        assert!(surface.fill_solid(&rect_zero_h, WHITE).is_ok());
        assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
    }

    #[test]
    fn surface_accessors_and_pixel_manipulation() {
        let mut buf = [0u32; 20 * 10];
        let mut surface = Surface::new(&mut buf, 20, 10).expect("surface");
        assert_eq!(surface.width(), 20);
        assert_eq!(surface.height(), 10);
        assert_eq!(surface.pixels().len(), 200);

        assert!(surface.set(5, 5, 0xFF123456));
        assert_eq!(surface.get(5, 5), Some(0xFF123456));
        assert_eq!(surface.pixels()[5 * 20 + 5], 0xFF123456);

        // Out of bounds get and set
        assert!(!surface.set(20, 5, 0xFFFFFFFF));
        assert!(!surface.set(5, 10, 0xFFFFFFFF));
        assert_eq!(surface.get(20, 5), None);
        assert_eq!(surface.get(5, 10), None);

        // Mutable slice manipulation
        surface.pixels_mut()[0] = 0xFFAABBCC;
        assert_eq!(surface.get(0, 0), Some(0xFFAABBCC));
    }

    #[test]
    fn test_blend_coverage_argb_ramp() {
        let fg = 0xFFFFFFFF;
        let bg = 0xFF000000;
        assert_eq!(blend_coverage_argb(fg, bg, 0), bg);
        assert_eq!(blend_coverage_argb(fg, bg, 255), fg);
        let mid = blend_coverage_argb(fg, bg, 128);
        let r = (mid >> 16) & 0xFF;
        assert!(r >= 127 && r <= 129);
    }

    #[test]
    fn test_draw_aa_rounded_rect_px_rendering() {
        let mut target = [0xFF000000u32; 50 * 50];
        draw_aa_rounded_rect_px(
            &mut target,
            50,
            50,
            50,
            5,
            5,
            40,
            40,
            8,
            0xFFFFFFFF,
            Some(0xFF888888),
            1,
        );
        // Center of rect should be white fill
        assert_eq!(target[25 * 50 + 25], 0xFFFFFFFF);
        // Outside rect should be black background
        assert_eq!(target[0], 0xFF000000);
        // Top-left corner outside arc should be untouched
        assert_eq!(target[5 * 50 + 5], 0xFF000000);
    }
}
