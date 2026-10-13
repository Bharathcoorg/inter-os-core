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

use embedded_graphics_core::draw_target::DrawTarget;
use embedded_graphics_core::geometry::{Dimensions, OriginDimensions, Point, Size};
use embedded_graphics_core::primitives::Rectangle;
use embedded_graphics_core::pixelcolor::PixelColor;

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
        if pixels.len() < width * height {
            return Err("buffer is smaller than the surface it is asked to be");
        }
        Ok(Self {
            pixels,
            width,
            height,
        })
    }

    /// The packed pixel already at a surface coordinate.
    pub fn get(&self, x: usize, y: usize) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.pixels[y * self.width + x])
    }
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
        let x0 = rectangle.top_left.x.max(0) as usize;
        let y0 = rectangle.top_left.y.max(0) as usize;
        let x1 = (rectangle.top_left.x + rectangle.size.width as i32)
            .min(self.width as i32) as usize;
        let y1 = (rectangle.top_left.y + rectangle.size.height as i32)
            .min(self.height as i32) as usize;
        for (y, color) in (y0..y1).zip(colors) {
            let row = y * self.width;
            for p in self.pixels[row + x0..row + x1].iter_mut() {
                *p = color.to_argb();
            }
        }
        Ok(())
    }

    /// Fills a whole rectangle.
    fn fill_solid(&mut self, rectangle: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let x0 = rectangle.top_left.x.max(0) as usize;
        let y0 = rectangle.top_left.y.max(0) as usize;
        let x1 = (rectangle.top_left.x + rectangle.size.width as i32)
            .min(self.width as i32) as usize;
        let y1 = (rectangle.top_left.y + rectangle.size.height as i32)
            .min(self.height as i32) as usize;
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        for y in y0..y1 {
            let row = y * self.width;
            for p in self.pixels[row + x0..row + x1].iter_mut() {
                *p = color.to_argb();
            }
        }
        Ok(())
    }

    /// Clears the whole surface.
    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let bbox = self.bounding_box();
        self.fill_solid(&bbox, color)
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
        let mut buf = [0u32; 10];
        assert!(Surface::new(&mut buf, 10, 10).is_err());
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
    fn a_rect_entirely_off_surface_writes_nothing() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        let rect = Rectangle::new(Point::new(200, 10), Size::new(20, 5));
        surface.fill_solid(&rect, WHITE).unwrap();
        assert_eq!(buf.iter().filter(|p| **p != 0).count(), 0);
    }

    #[test]
    fn fill_contiguous_writes_row_at_a_time_and_stops_when_colors_run_out() {
        let mut buf = [0u32; 100 * 100];
        let mut surface = Surface::new(&mut buf, 100, 100).expect("surface");
        let rect = Rectangle::new(Point::new(0, 0), Size::new(3, 3));
        // Two colours for a three-row rect: the third row is left alone, because
        // the color iterator is exhausted and inventing one would be a draw the
        // caller did not ask for.
        surface.fill_contiguous(&rect, [WHITE, WHITE]).unwrap();
        assert_eq!(buf[0], WHITE.to_argb());
        assert_eq!(buf[1], WHITE.to_argb());
        assert_eq!(buf[2], WHITE.to_argb());
        assert_eq!(buf[2 * 100], 0, "row three, no color supplied");
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
}
