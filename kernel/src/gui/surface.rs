// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! High-Performance Double-Buffered Graphics Surface & Dirty-Region Blitter
//!
//! Provides off-screen pixel buffer rendering with automatic dirty-region tracking,
//! smooth linear gradients, alpha blending (glassmorphism), and fast memory blitting
//! to the physical video framebuffer.

use alloc::vec::Vec;
use crate::framebuffer::{FramebufferWriter, FONT_8X8};

/// 32-bit RGBA Color
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Converts to standard 32-bit 0xAARRGGBB packed integer.
    #[inline]
    pub const fn to_u32(&self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    /// Reconstructs Color from 32-bit 0xAARRGGBB packed integer.
    #[inline]
    pub const fn from_u32(val: u32) -> Self {
        Self {
            a: (val >> 24) as u8,
            r: (val >> 16) as u8,
            g: (val >> 8) as u8,
            b: val as u8,
        }
    }

    // Pre-defined System Palette Constants
    pub const OBSIDIAN: Color = Color::rgb(11, 15, 25);
    pub const FRAME_BG: Color = Color::rgb(16, 22, 34);
    pub const BEZEL: Color = Color::rgb(38, 48, 68);
    pub const CARD_BG: Color = Color::rgb(24, 32, 48);
    pub const CARD_BORDER: Color = Color::rgb(45, 58, 82);
    pub const CYAN: Color = Color::rgb(0, 229, 255);
    pub const GREEN: Color = Color::rgb(16, 185, 129);
    pub const PURPLE: Color = Color::rgb(168, 85, 247);
    pub const AMBER: Color = Color::rgb(245, 158, 11);
    pub const WHITE: Color = Color::rgb(248, 250, 252);
    pub const MUTED: Color = Color::rgb(148, 163, 184);
    pub const DARK_SLATE: Color = Color::rgb(5, 8, 15);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
}

/// 2D Rectangle Geometry
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub const fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self { x, y, width, height }
    }

    /// Returns true if coordinate `(px, py)` falls within this rectangle.
    #[inline]
    pub fn contains(&self, px: usize, py: usize) -> bool {
        px >= self.x && px < self.x + self.width && py >= self.y && py < self.y + self.height
    }

    /// Returns true if this rectangle intersects with `other`.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    /// Computes the minimal bounding box enclosing both rectangles.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.width == 0 || self.height == 0 {
            return *other;
        }
        if other.width == 0 || other.height == 0 {
            return *self;
        }

        let min_x = self.x.min(other.x);
        let min_y = self.y.min(other.y);
        let max_x = (self.x + self.width).max(other.x + other.width);
        let max_y = (self.y + self.height).max(other.y + other.height);

        Rect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        }
    }

    /// Clamps rectangle within bounds `(max_w, max_h)`.
    pub fn clamp(&self, max_w: usize, max_h: usize) -> Rect {
        let x = self.x.min(max_w);
        let y = self.y.min(max_h);
        let width = (self.width).min(max_w.saturating_sub(x));
        let height = (self.height).min(max_h.saturating_sub(y));
        Rect { x, y, width, height }
    }
}

/// Double-Buffered Graphical Rendering Surface
pub struct Surface {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub pixels: Vec<u32>,
    pub dirty_rect: Option<Rect>,
}

impl Surface {
    /// Creates a new surface initialized with transparent black pixels.
    pub fn new(width: usize, height: usize) -> Self {
        let size = width * height;
        Self {
            width,
            height,
            stride: width,
            pixels: alloc::vec![0u32; size],
            dirty_rect: Some(Rect::new(0, 0, width, height)),
        }
    }

    /// Marks a bounding region as modified (dirty) so the blitter updates it.
    pub fn mark_dirty(&mut self, rect: Rect) {
        let clamped = rect.clamp(self.width, self.height);
        if clamped.width == 0 || clamped.height == 0 {
            return;
        }
        self.dirty_rect = match self.dirty_rect {
            Some(existing) => Some(existing.union(&clamped)),
            None => Some(clamped),
        };
    }

    /// Resets the dirty region tracker.
    pub fn clear_dirty(&mut self) {
        self.dirty_rect = None;
    }

    /// Reads pixel color at `(x, y)`.
    #[inline]
    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x < self.width && y < self.height {
            Color::from_u32(self.pixels[y * self.stride + x])
        } else {
            Color::TRANSPARENT
        }
    }

    /// Writes pixel color at `(x, y)` and tracks dirty status.
    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x < self.width && y < self.height {
            self.pixels[y * self.stride + x] = color.to_u32();
            self.mark_dirty(Rect::new(x, y, 1, 1));
        }
    }

    /// Fast pixel write without immediately expanding dirty bounding box.
    #[inline]
    pub fn set_pixel_raw(&mut self, x: usize, y: usize, color: Color) {
        if x < self.width && y < self.height {
            self.pixels[y * self.stride + x] = color.to_u32();
        }
    }

    /// Clears the entire surface with a uniform solid color.
    pub fn clear(&mut self, color: Color) {
        let val = color.to_u32();
        for p in self.pixels.iter_mut() {
            *p = val;
        }
        self.dirty_rect = Some(Rect::new(0, 0, self.width, self.height));
    }

    /// Fills a solid rectangle with `color`.
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let clamped = rect.clamp(self.width, self.height);
        if clamped.width == 0 || clamped.height == 0 {
            return;
        }

        let val = color.to_u32();
        for cy in clamped.y..(clamped.y + clamped.height) {
            let row_start = cy * self.stride + clamped.x;
            let row_slice = &mut self.pixels[row_start..(row_start + clamped.width)];
            for p in row_slice.iter_mut() {
                *p = val;
            }
        }
        self.mark_dirty(clamped);
    }

    /// Draws a hollow rectangular outline with specified border thickness.
    pub fn draw_rect_outline(&mut self, rect: Rect, thickness: usize, color: Color) {
        if thickness == 0 {
            return;
        }
        // Top
        self.fill_rect(Rect::new(rect.x, rect.y, rect.width, thickness), color);
        // Bottom
        self.fill_rect(
            Rect::new(rect.x, rect.y + rect.height.saturating_sub(thickness), rect.width, thickness),
            color,
        );
        // Left
        self.fill_rect(Rect::new(rect.x, rect.y, thickness, rect.height), color);
        // Right
        self.fill_rect(
            Rect::new(rect.x + rect.width.saturating_sub(thickness), rect.y, thickness, rect.height),
            color,
        );
    }

    /// Draws an ASCII character scaled by `scale` factor using the embedded 8x8 font.
    pub fn draw_char_scaled(
        &mut self,
        x: usize,
        y: usize,
        c: char,
        scale: usize,
        fg: Color,
        bg: Option<Color>,
    ) {
        let ascii_val = c as u32;
        let glyph_idx = if (32..=126).contains(&ascii_val) {
            (ascii_val - 32) as usize
        } else {
            0
        };
        let glyph = FONT_8X8[glyph_idx];

        for font_row in 0..8 {
            let row_byte = glyph[font_row];
            for font_col in 0..8 {
                let is_fg = ((row_byte >> (7 - font_col)) & 1) == 1;
                if is_fg {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            self.set_pixel_raw(x + font_col * scale + dx, y + font_row * scale + dy, fg);
                        }
                    }
                } else if let Some(bg_color) = bg {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            self.set_pixel_raw(x + font_col * scale + dx, y + font_row * scale + dy, bg_color);
                        }
                    }
                }
            }
        }
        self.mark_dirty(Rect::new(x, y, 8 * scale, 8 * scale));
    }

    /// Draws a text string at `(x, y)`.
    pub fn draw_string(
        &mut self,
        x: usize,
        y: usize,
        text: &str,
        scale: usize,
        fg: Color,
        bg: Option<Color>,
    ) {
        let mut cur_x = x;
        let char_w = 8 * scale;
        for c in text.chars() {
            self.draw_char_scaled(cur_x, y, c, scale, fg, bg);
            cur_x += char_w;
        }
    }

    /// Renders a vertical linear gradient from `top_color` to `bottom_color`.
    pub fn draw_gradient_v(&mut self, rect: Rect, top_color: Color, bottom_color: Color) {
        let clamped = rect.clamp(self.width, self.height);
        if clamped.width == 0 || clamped.height == 0 {
            return;
        }

        for dy in 0..clamped.height {
            let t = (dy * 256) / clamped.height;
            let inv_t = 256 - t;

            let r = ((top_color.r as usize * inv_t + bottom_color.r as usize * t) / 256) as u8;
            let g = ((top_color.g as usize * inv_t + bottom_color.g as usize * t) / 256) as u8;
            let b = ((top_color.b as usize * inv_t + bottom_color.b as usize * t) / 256) as u8;
            let a = ((top_color.a as usize * inv_t + bottom_color.a as usize * t) / 256) as u8;
            let line_color = Color { r, g, b, a };

            let row_start = (clamped.y + dy) * self.stride + clamped.x;
            let row_slice = &mut self.pixels[row_start..(row_start + clamped.width)];
            let val = line_color.to_u32();
            for p in row_slice.iter_mut() {
                *p = val;
            }
        }
        self.mark_dirty(clamped);
    }

    /// Alpha-blends a semi-transparent color over existing pixels in `rect` (Glassmorphism effect).
    pub fn blend_rect(&mut self, rect: Rect, color: Color) {
        let clamped = rect.clamp(self.width, self.height);
        if clamped.width == 0 || clamped.height == 0 || color.a == 0 {
            return;
        }

        let alpha = color.a as u32;
        let inv_alpha = 255 - alpha;

        for cy in clamped.y..(clamped.y + clamped.height) {
            let row_start = cy * self.stride + clamped.x;
            let row_slice = &mut self.pixels[row_start..(row_start + clamped.width)];
            for p in row_slice.iter_mut() {
                let dst = Color::from_u32(*p);
                let out_r = ((color.r as u32 * alpha + dst.r as u32 * inv_alpha) / 255) as u8;
                let out_g = ((color.g as u32 * alpha + dst.g as u32 * inv_alpha) / 255) as u8;
                let out_b = ((color.b as u32 * alpha + dst.b as u32 * inv_alpha) / 255) as u8;
                *p = Color::rgb(out_r, out_g, out_b).to_u32();
            }
        }
        self.mark_dirty(clamped);
    }

    /// Blits dirty regions of the surface to the hardware framebuffer.
    pub fn blt_to_framebuffer(
        &mut self,
        fb: &mut FramebufferWriter,
        dest_x: usize,
        dest_y: usize,
    ) -> Option<Rect> {
        let dirty = self.dirty_rect.take()?;
        for cy in dirty.y..(dirty.y + dirty.height) {
            let row_start = cy * self.stride + dirty.x;
            for cx in 0..dirty.width {
                let val = self.pixels[row_start + cx];
                let color = Color::from_u32(val);
                fb.set_pixel(dest_x + dirty.x + cx, dest_y + cy, (color.r, color.g, color.b));
            }
        }
        Some(dirty)
    }
}
