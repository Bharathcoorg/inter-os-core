//! High-performance display-list and horizontal band rasterizer.
//!
//! Eliminates per-pixel re-layout costs by recording positioned drawing commands
//! into a stack-allocated or static buffer, then evaluating each scanline band
//! in one linear sweep. Zero heap allocation.

use crate::font;

pub const MAX_TEXT_BYTES: usize = 64;

/// 2D rectangle in surface coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Self {
        Self { x, y, w, h }
    }
}

/// Positioned drawing commands supported by the display list.
#[derive(Clone, Copy)]
pub enum DrawCmd {
    Rect {
        rect: Rect,
        fill: u32,
        radius: usize,
        border: Option<u32>,
    },
    Text {
        x: usize,
        y: usize,
        scale: usize,
        color: u32,
        bold: bool,
        len: usize,
        bytes: [u8; MAX_TEXT_BYTES],
    },
}

/// Fixed-capacity, allocation-free display list buffer.
pub struct DisplayList<const CAP: usize> {
    commands: [Option<DrawCmd>; CAP],
    len: usize,
    pub bg_color: u32,
}

impl<const CAP: usize> DisplayList<CAP> {
    pub const fn new(bg_color: u32) -> Self {
        Self {
            commands: [None; CAP],
            len: 0,
            bg_color,
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Pushes a filled rectangle with optional rounded corners and border.
    pub fn push_rect(&mut self, rect: Rect, fill: u32, radius: usize, border: Option<u32>) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.commands[self.len] = Some(DrawCmd::Rect { rect, fill, radius, border });
        self.len += 1;
        true
    }

    /// Pushes a line of anti-aliased text.
    pub fn push_text(&mut self, x: usize, y: usize, scale: usize, text: &[u8], color: u32, bold: bool) -> bool {
        if self.len >= CAP {
            return false;
        }
        let mut bytes = [0u8; MAX_TEXT_BYTES];
        let n = text.len().min(MAX_TEXT_BYTES);
        bytes[..n].copy_from_slice(&text[..n]);
        self.commands[self.len] = Some(DrawCmd::Text { x, y, scale, color, bold, len: n, bytes });
        self.len += 1;
        true
    }

    /// Rasterizes horizontal scanout rows in range `[y0, y1)` across width `w` into `out`.
    pub fn rasterize_band(&self, y0: usize, y1: usize, w: usize, out: &mut [u32]) {
        let row_count = y1.saturating_sub(y0);
        if row_count == 0 || w == 0 || out.len() < row_count * w {
            return;
        }
        for px in out[..row_count * w].iter_mut() {
            *px = self.bg_color;
        }

        // Iterate through active commands affecting this band
        for cmd_opt in &self.commands[..self.len] {
            let Some(cmd) = cmd_opt else { continue; };
            match cmd {
                DrawCmd::Rect { rect, fill, radius, border } => {
                    let ry0 = rect.y.max(y0);
                    let ry1 = (rect.y + rect.h).min(y1);
                    if ry1 <= ry0 { continue; }
                    let rx0 = rect.x.min(w);
                    let rx1 = (rect.x + rect.w).min(w);
                    if rx1 <= rx0 { continue; }

                    for y in ry0..ry1 {
                        let out_row_idx = (y - y0) * w;
                        for x in rx0..rx1 {
                            if *radius > 0 {
                                let r = *radius;
                                let is_left = x < rect.x + r;
                                let is_right = x >= rect.x + rect.w.saturating_sub(r);
                                let is_top = y < rect.y + r;
                                let is_bottom = y >= rect.y + rect.h.saturating_sub(r);

                                if (is_left || is_right) && (is_top || is_bottom) {
                                    let cx = if is_left { rect.x + r } else { rect.x + rect.w - r - 1 };
                                    let cy = if is_top { rect.y + r } else { rect.y + rect.h - r - 1 };
                                    let dx = (x as i32) - (cx as i32);
                                    let dy = (y as i32) - (cy as i32);
                                    if dx * dx + dy * dy > (r as i32) * (r as i32) {
                                        continue;
                                    }
                                }
                            }
                            if let Some(b_color) = border {
                                if x == rect.x || x == rect.x + rect.w - 1 || y == rect.y || y == rect.y + rect.h - 1 {
                                    out[out_row_idx + x] = *b_color;
                                    continue;
                                }
                            }
                            out[out_row_idx + x] = *fill;
                        }
                    }
                }
                DrawCmd::Text { x, y, scale, color, bold, len, bytes } => {
                    let th = font::line_height(*scale);
                    let ty0 = (*y).max(y0);
                    let ty1 = (*y + th).min(y1);
                    if ty1 <= ty0 { continue; }
                    let tw = font::measure_weighted(&bytes[..*len], *scale, *bold);
                    let tx0 = (*x).min(w);
                    let tx1 = (*x + tw).min(w);
                    if tx1 <= tx0 { continue; }

                    for py in ty0..ty1 {
                        let out_row_idx = (py - y0) * w;
                        for px in tx0..tx1 {
                            let cur_bg = out[out_row_idx + px];
                            if let Some(c) = font::test_text_aa(px, py, *x, *y, *scale, &bytes[..*len], *color, cur_bg, *bold) {
                                out[out_row_idx + px] = c;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_list_push_and_rasterize() {
        let mut dl = DisplayList::<16>::new(0xFF000000); // black background
        assert_eq!(dl.len(), 0);

        let rect = Rect::new(10, 10, 50, 20);
        assert!(dl.push_rect(rect, 0xFFFFFFFF, 0, None));
        assert_eq!(dl.len(), 1);

        assert!(dl.push_text(12, 12, 1, b"Test", 0xFFFF0000, false));
        assert_eq!(dl.len(), 2);

        let mut band = [0u32; 100 * 30]; // 100 wide, 30 high: y 0..30
        dl.rasterize_band(0, 30, 100, &mut band);

        // Outside rect is black
        assert_eq!(band[0], 0xFF000000);
        // Inside rect is white or text ink
        assert_ne!(band[15 * 100 + 20], 0xFF000000);
    }

    #[test]
    fn test_display_list_capacity_overflow() {
        let mut dl = DisplayList::<2>::new(0xFF000000);
        assert!(dl.push_rect(Rect::new(0, 0, 10, 10), 0xFFFFFFFF, 0, None));
        assert!(dl.push_rect(Rect::new(10, 10, 10, 10), 0xFFFFFFFF, 0, None));
        // Third push must fail without panic
        assert!(!dl.push_rect(Rect::new(20, 20, 10, 10), 0xFFFFFFFF, 0, None));
        assert_eq!(dl.len(), 2);
    }
}
