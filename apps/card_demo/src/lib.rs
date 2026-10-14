#![no_std]

pub use interos_std as std;
use std::design::draw_target::SurfaceColor;

pub const W: usize = 720;
pub const H: usize = 1440;
pub const BAND_H: usize = 240;
pub const NUM_BANDS: usize = H / BAND_H; // 6 bands = 1440 rows

pub const SCREEN_BG: SurfaceColor = SurfaceColor::from_rgb(0x11, 0x12, 0x14);
pub const CARD_BG: SurfaceColor = SurfaceColor::from_rgb(0x1F, 0x23, 0x28);
pub const CARD_BORDER: SurfaceColor = SurfaceColor::from_rgb(0x35, 0x38, 0x3D);
pub const ACCENT_BLUE: SurfaceColor = SurfaceColor::from_rgb(0x25, 0x63, 0xEB);
pub const BADGE_BG: SurfaceColor = SurfaceColor::from_rgb(0x16, 0x18, 0x1A);

/// Computes the vertical row extent (inclusive start, exclusive end) of a given band index.
pub fn band_y_range(band: usize) -> (usize, usize) {
    let y0 = band * BAND_H;
    let y1 = (y0 + BAND_H).min(H);
    (y0, y1)
}

/// Checks if a rectangle with [rect_y, rect_y + rect_h) intersects with the given band.
pub fn band_overlaps_rect(band: usize, rect_y: usize, rect_h: usize) -> bool {
    let (by0, by1) = band_y_range(band);
    let ry1 = rect_y + rect_h;
    rect_y < by1 && ry1 > by0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_band_geometry() {
        assert_eq!(NUM_BANDS, 6);
        assert_eq!(band_y_range(0), (0, 240));
        assert_eq!(band_y_range(1), (240, 480));
        assert_eq!(band_y_range(5), (1200, 1440));
    }

    #[test]
    fn test_band_overlap() {
        // Card at y=260, h=560 -> extent [260, 820)
        // Band 0: [0, 240) -> no overlap
        assert!(!band_overlaps_rect(0, 260, 560));
        // Band 1: [240, 480) -> overlaps
        assert!(band_overlaps_rect(1, 260, 560));
        // Band 2: [480, 720) -> overlaps
        assert!(band_overlaps_rect(2, 260, 560));
        // Band 3: [720, 960) -> overlaps
        assert!(band_overlaps_rect(3, 260, 560));
        // Band 4: [960, 1200) -> no overlap
        assert!(!band_overlaps_rect(4, 260, 560));
    }

    #[test]
    fn test_surface_colors() {
        assert_eq!(SCREEN_BG.0, 0xFF111214);
        assert_eq!(ACCENT_BLUE.0, 0xFF2563EB);
    }
}
