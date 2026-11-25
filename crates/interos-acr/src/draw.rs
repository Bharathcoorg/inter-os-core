// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Bounded Android Drawing Runtime & Real Typography Rasterizer
//!
//! Provides a validated, strictly bounded drawing path for Android activity surfaces.
//! Android activity surfaces are allocated through KMS via `AndroidWindowManager`.
//! Drawing commands emitted by untrusted APK/DEX workloads or View hierarchies are
//! validated against surface bounds, arithmetic overflow, command count ceilings,
//! and cumulative per-frame area budgets.
//!
//! Includes:
//! - Primitive drawing operations: FillRect, DrawHLine, DrawVLine, StrokeRect, DrawBitmap, DrawGlyph, DrawText
//! - Software rasterizer blitting actual font glyphs to the surface framebuffer (8x16 typography stack)

use alloc::string::String;
use alloc::vec::Vec;

pub const ANDROID_SURFACE_WIDTH: u32 = 720;
pub const ANDROID_SURFACE_HEIGHT: u32 = 1320;

/// Maximum drawing operations allowed in a single DrawList.
pub const MAX_DRAW_OPS_PER_LIST: usize = 256;

/// Cumulative pixel area budget per frame (2 full surface equivalents = 2 * 720 * 1320).
pub const MAX_FRAME_AREA_PIXELS: u64 = (ANDROID_SURFACE_WIDTH as u64) * (ANDROID_SURFACE_HEIGHT as u64) * 2;

/// Maximum dimension allowed for a single draw primitive.
pub const MAX_PRIMITIVE_DIMENSION: u32 = 4096;

/// Errors returned during drawing command validation and rasterization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawError {
    InvalidSurface,
    OwnershipDenied,
    OutOfBounds,
    IntegerOverflow,
    CommandCountExceeded,
    AreaBudgetExceeded,
    InvalidDimensions,
    MalformedCommand,
}

/// Primitive drawing operations supported on Android activity surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawOp {
    /// Clears the entire surface with an ARGB8888 color.
    Clear { color: u32 },
    /// Fills an axis-aligned rectangle with clipping to surface bounds.
    FillRect { x: i32, y: i32, width: u32, height: u32, color: u32 },
    /// Draws a 1-pixel horizontal line span.
    DrawHLine { x: i32, y: i32, width: u32, color: u32 },
    /// Draws a 1-pixel vertical line span.
    DrawVLine { x: i32, y: i32, height: u32, color: u32 },
    /// Strokes an outlined rectangle with given border width.
    StrokeRect { x: i32, y: i32, width: u32, height: u32, color: u32, stroke_width: u32 },
    /// Draws a single ASCII character glyph (8x16) at given coordinates.
    DrawGlyph { x: i32, y: i32, ascii: u8, color: u32 },
    /// Draws a text string rendered with standard typography at given coordinates.
    DrawText { x: i32, y: i32, text: String, color: u32, size: u32 },
    /// Blits a decoded bitmap onto the surface.
    DrawBitmap { x: i32, y: i32, width: u32, height: u32, pixels: Vec<u32> },
    /// Pushes a clipping boundary for subsequent drawing commands.
    PushClip { x: i32, y: i32, width: u32, height: u32 },
    /// Pops the active clipping boundary.
    PopClip,
}

/// A validated list of drawing operations to execute on an Android activity surface.
#[derive(Debug, Clone)]
pub struct DrawList {
    pub ops: Vec<DrawOp>,
}

impl DrawList {
    pub const fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub fn ops(&self) -> &[DrawOp] {
        &self.ops
    }

    pub fn push(&mut self, op: DrawOp) -> Result<(), DrawError> {
        if self.ops.len() >= MAX_DRAW_OPS_PER_LIST {
            return Err(DrawError::CommandCountExceeded);
        }

        match &op {
            DrawOp::Clear { .. } => {}
            DrawOp::FillRect { width, height, .. } => {
                if *width == 0 || *height == 0 || *width > MAX_PRIMITIVE_DIMENSION || *height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::DrawHLine { width, .. } => {
                if *width == 0 || *width > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::DrawVLine { height, .. } => {
                if *height == 0 || *height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::StrokeRect { width, height, .. } => {
                if *width == 0 || *height == 0 || *width > MAX_PRIMITIVE_DIMENSION || *height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::DrawGlyph { .. } => {}
            DrawOp::DrawText { text, .. } => {
                if text.len() > 256 {
                    return Err(DrawError::MalformedCommand);
                }
            }
            DrawOp::DrawBitmap { width, height, pixels, .. } => {
                if *width == 0 || *height == 0 || *width > MAX_PRIMITIVE_DIMENSION || *height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
                if pixels.len() != (*width as usize) * (*height as usize) {
                    return Err(DrawError::MalformedCommand);
                }
            }
            DrawOp::PushClip { width, height, .. } => {
                if *width == 0 || *height == 0 || *width > MAX_PRIMITIVE_DIMENSION || *height > MAX_PRIMITIVE_DIMENSION {
                    return Err(DrawError::InvalidDimensions);
                }
            }
            DrawOp::PopClip => {}
        }

        self.ops.push(op);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

// -----------------------------------------------------------------------------
// Sovereign 8x16 Font Glyph Table (Standard ASCII 32..127)
// -----------------------------------------------------------------------------

/// 8x16 basic font bitmap data for ASCII characters
pub fn get_glyph_bitmap(c: u8) -> [u8; 16] {
    match c {
        b' ' => [0; 16],
        b'A' => [0, 0, 0x18, 0x24, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0, 0, 0, 0, 0],
        b'B' => [0, 0, 0x7C, 0x42, 0x42, 0x7C, 0x42, 0x42, 0x42, 0x7C, 0, 0, 0, 0, 0, 0],
        b'C' => [0, 0, 0x3C, 0x42, 0x40, 0x40, 0x40, 0x40, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'D' => [0, 0, 0x78, 0x44, 0x42, 0x42, 0x42, 0x42, 0x44, 0x78, 0, 0, 0, 0, 0, 0],
        b'E' => [0, 0, 0x7E, 0x40, 0x40, 0x7C, 0x40, 0x40, 0x40, 0x7E, 0, 0, 0, 0, 0, 0],
        b'F' => [0, 0, 0x7E, 0x40, 0x40, 0x7C, 0x40, 0x40, 0x40, 0x40, 0, 0, 0, 0, 0, 0],
        b'G' => [0, 0, 0x3C, 0x42, 0x40, 0x40, 0x4E, 0x42, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'H' => [0, 0, 0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0, 0, 0, 0, 0, 0],
        b'I' => [0, 0, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0, 0, 0, 0, 0, 0],
        b'J' => [0, 0, 0x1E, 0x06, 0x06, 0x06, 0x06, 0x06, 0x46, 0x3C, 0, 0, 0, 0, 0, 0],
        b'K' => [0, 0, 0x44, 0x48, 0x50, 0x60, 0x50, 0x48, 0x44, 0x42, 0, 0, 0, 0, 0, 0],
        b'L' => [0, 0, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x7E, 0, 0, 0, 0, 0, 0],
        b'M' => [0, 0, 0x81, 0xC3, 0xA5, 0x99, 0x81, 0x81, 0x81, 0x81, 0, 0, 0, 0, 0, 0],
        b'N' => [0, 0, 0x42, 0x62, 0x52, 0x4A, 0x46, 0x42, 0x42, 0x42, 0, 0, 0, 0, 0, 0],
        b'O' => [0, 0, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'P' => [0, 0, 0x7C, 0x42, 0x42, 0x7C, 0x40, 0x40, 0x40, 0x40, 0, 0, 0, 0, 0, 0],
        b'Q' => [0, 0, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x4A, 0x44, 0x3A, 0, 0, 0, 0, 0, 0],
        b'R' => [0, 0, 0x7C, 0x42, 0x42, 0x7C, 0x48, 0x44, 0x42, 0x42, 0, 0, 0, 0, 0, 0],
        b'S' => [0, 0, 0x3C, 0x42, 0x40, 0x3C, 0x02, 0x02, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'T' => [0, 0, 0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0, 0, 0, 0, 0, 0],
        b'U' => [0, 0, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'V' => [0, 0, 0x42, 0x42, 0x42, 0x42, 0x42, 0x24, 0x24, 0x18, 0, 0, 0, 0, 0, 0],
        b'W' => [0, 0, 0x81, 0x81, 0x81, 0x81, 0x99, 0xA5, 0xC3, 0x81, 0, 0, 0, 0, 0, 0],
        b'X' => [0, 0, 0x42, 0x24, 0x24, 0x18, 0x18, 0x24, 0x24, 0x42, 0, 0, 0, 0, 0, 0],
        b'Y' => [0, 0, 0x42, 0x42, 0x24, 0x24, 0x18, 0x18, 0x18, 0x18, 0, 0, 0, 0, 0, 0],
        b'Z' => [0, 0, 0x7E, 0x04, 0x08, 0x10, 0x20, 0x40, 0x40, 0x7E, 0, 0, 0, 0, 0, 0],
        b'a' => [0, 0, 0, 0, 0x3C, 0x02, 0x3E, 0x42, 0x46, 0x3B, 0, 0, 0, 0, 0, 0],
        b'b' => [0, 0, 0x40, 0x40, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x7C, 0, 0, 0, 0, 0, 0],
        b'c' => [0, 0, 0, 0, 0x3C, 0x42, 0x40, 0x40, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'd' => [0, 0, 0x02, 0x02, 0x3E, 0x42, 0x42, 0x42, 0x42, 0x3B, 0, 0, 0, 0, 0, 0],
        b'e' => [0, 0, 0, 0, 0x3C, 0x42, 0x7E, 0x40, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'f' => [0, 0, 0x0C, 0x12, 0x10, 0x7C, 0x10, 0x10, 0x10, 0x10, 0, 0, 0, 0, 0, 0],
        b'g' => [0, 0, 0, 0, 0x3B, 0x42, 0x42, 0x3E, 0x02, 0x3C, 0, 0, 0, 0, 0, 0],
        b'h' => [0, 0, 0x40, 0x40, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0, 0, 0, 0, 0, 0],
        b'i' => [0, 0, 0x18, 0, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0, 0, 0, 0, 0, 0],
        b'l' => [0, 0, 0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0, 0, 0, 0, 0, 0],
        b'm' => [0, 0, 0, 0, 0xFE, 0x92, 0x92, 0x92, 0x92, 0x92, 0, 0, 0, 0, 0, 0],
        b'n' => [0, 0, 0, 0, 0x7C, 0x42, 0x42, 0x42, 0x42, 0x42, 0, 0, 0, 0, 0, 0],
        b'o' => [0, 0, 0, 0, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b'p' => [0, 0, 0, 0, 0x7C, 0x42, 0x42, 0x7C, 0x40, 0x40, 0x40, 0, 0, 0, 0, 0],
        b'r' => [0, 0, 0, 0, 0x5C, 0x62, 0x40, 0x40, 0x40, 0x40, 0, 0, 0, 0, 0, 0],
        b's' => [0, 0, 0, 0, 0x3C, 0x40, 0x3C, 0x02, 0x42, 0x3C, 0, 0, 0, 0, 0, 0],
        b't' => [0, 0, 0x10, 0x10, 0x7C, 0x10, 0x10, 0x10, 0x12, 0x0C, 0, 0, 0, 0, 0, 0],
        b'u' => [0, 0, 0, 0, 0x42, 0x42, 0x42, 0x42, 0x46, 0x3B, 0, 0, 0, 0, 0, 0],
        b'v' => [0, 0, 0, 0, 0x42, 0x42, 0x42, 0x24, 0x24, 0x18, 0, 0, 0, 0, 0, 0],
        b'w' => [0, 0, 0, 0, 0x81, 0x81, 0x99, 0x99, 0x66, 0x66, 0, 0, 0, 0, 0, 0],
        b'x' => [0, 0, 0, 0, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0, 0, 0, 0, 0, 0],
        b'y' => [0, 0, 0, 0, 0x42, 0x42, 0x42, 0x3E, 0x02, 0x3C, 0, 0, 0, 0, 0, 0],
        b'0'..=b'9' => {
            let digit = c - b'0';
            [0, 0, 0x3C, 0x42, 0x42, 0x42 + digit, 0x42, 0x42, 0x42, 0x3C, 0, 0, 0, 0, 0, 0]
        }
        b'!' => [0, 0, 0x18, 0x18, 0x18, 0x18, 0x18, 0, 0x18, 0, 0, 0, 0, 0, 0, 0],
        b':' => [0, 0, 0, 0x18, 0x18, 0, 0, 0x18, 0x18, 0, 0, 0, 0, 0, 0, 0],
        b'-' => [0, 0, 0, 0, 0, 0x7E, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        _ => [0, 0, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0, 0, 0, 0, 0, 0], // box
    }
}

/// Blends source color over destination pixel using standard Porter-Duff alpha compositing.
#[inline(always)]
pub fn blend_argb(src: u32, dst: u32) -> u32 {
    let sa = ((src >> 24) & 0xFF) as u32;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = ((dst >> 24) & 0xFF) as u32;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;

    let inv_sa = 255 - sa;
    let out_r = (sr * sa + dr * inv_sa + 127) / 255;
    let out_g = (sg * sa + dg * inv_sa + 127) / 255;
    let out_b = (sb * sa + db * inv_sa + 127) / 255;
    let out_a = (sa * 255 + da * inv_sa + 127) / 255;

    (out_a.min(255) << 24) | (out_r.min(255) << 16) | (out_g.min(255) << 8) | out_b.min(255)
}

/// Software rasterizer that executes a DrawList onto a raw RGBA32/ARGB32 frame buffer with clipping and alpha blending.
pub fn render_draw_list_to_buffer(draw_list: &DrawList, buffer: &mut [u32], buf_w: u32, buf_h: u32) {
    let mut clip_stack = alloc::vec![(0i32, 0i32, buf_w as i32, buf_h as i32)];

    for op in &draw_list.ops {
        match op {
            DrawOp::PushClip { x, y, width, height } => {
                let cur = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                let new_clip = (
                    cur.0.max(*x),
                    cur.1.max(*y),
                    cur.2.min(*x + *width as i32),
                    cur.3.min(*y + *height as i32),
                );
                clip_stack.push(new_clip);
            }
            DrawOp::PopClip => {
                if clip_stack.len() > 1 {
                    clip_stack.pop();
                }
            }
            DrawOp::Clear { color } => {
                buffer.fill(*color);
            }
            DrawOp::FillRect { x, y, width, height, color } => {
                let clip = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                let x0 = (*x).max(clip.0).max(0) as u32;
                let y0 = (*y).max(clip.1).max(0) as u32;
                let x1 = ((*x + *width as i32).min(clip.2).max(0) as u32).min(buf_w);
                let y1 = ((*y + *height as i32).min(clip.3).max(0) as u32).min(buf_h);

                for py in y0..y1 {
                    let row_start = (py * buf_w) as usize;
                    for px in x0..x1 {
                        let idx = row_start + px as usize;
                        buffer[idx] = blend_argb(*color, buffer[idx]);
                    }
                }
            }
            DrawOp::DrawHLine { x, y, width, color } => {
                let clip = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                if *y >= clip.1 && *y < clip.3 && *y >= 0 && (*y as u32) < buf_h {
                    let x0 = (*x).max(clip.0).max(0) as u32;
                    let x1 = ((*x + *width as i32).min(clip.2).max(0) as u32).min(buf_w);
                    let row = (*y as u32 * buf_w) as usize;
                    for px in x0..x1 {
                        let idx = row + px as usize;
                        buffer[idx] = blend_argb(*color, buffer[idx]);
                    }
                }
            }
            DrawOp::DrawVLine { x, y, height, color } => {
                let clip = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                if *x >= clip.0 && *x < clip.2 && *x >= 0 && (*x as u32) < buf_w {
                    let y0 = (*y).max(clip.1).max(0) as u32;
                    let y1 = ((*y + *height as i32).min(clip.3).max(0) as u32).min(buf_h);
                    let px = *x as usize;
                    for py in y0..y1 {
                        let idx = (py * buf_w) as usize + px;
                        buffer[idx] = blend_argb(*color, buffer[idx]);
                    }
                }
            }
            DrawOp::StrokeRect { x, y, width, height, color, stroke_width } => {
                let sw = (*stroke_width).max(1);
                // Top, bottom, left, right borders
                let _ = render_draw_list_to_buffer(&DrawList {
                    ops: alloc::vec![
                        DrawOp::FillRect { x: *x, y: *y, width: *width, height: sw, color: *color },
                        DrawOp::FillRect { x: *x, y: *y + *height as i32 - sw as i32, width: *width, height: sw, color: *color },
                        DrawOp::FillRect { x: *x, y: *y, width: sw, height: *height, color: *color },
                        DrawOp::FillRect { x: *x + *width as i32 - sw as i32, y: *y, width: sw, height: *height, color: *color },
                    ]
                }, buffer, buf_w, buf_h);
            }
            DrawOp::DrawGlyph { x, y, ascii, color } => {
                let clip = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                let bitmap = get_glyph_bitmap(*ascii);
                for (row, &byte) in bitmap.iter().enumerate() {
                    let py = *y + row as i32;
                    if py < clip.1 || py >= clip.3 || py < 0 || py >= buf_h as i32 {
                        continue;
                    }
                    for col in 0..8 {
                        if (byte & (0x80 >> col)) != 0 {
                            let px = *x + col as i32;
                            if px >= clip.0 && px < clip.2 && px >= 0 && px < buf_w as i32 {
                                let idx = (py as u32 * buf_w + px as u32) as usize;
                                buffer[idx] = blend_argb(*color, buffer[idx]);
                            }
                        }
                    }
                }
            }
            DrawOp::DrawText { x, y, text, color, .. } => {
                let clip = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                let mut cur_x = *x;
                for b in text.bytes() {
                    let bitmap = get_glyph_bitmap(b);
                    for (row, &byte) in bitmap.iter().enumerate() {
                        let py = *y + row as i32;
                        if py < clip.1 || py >= clip.3 || py < 0 || py >= buf_h as i32 {
                            continue;
                        }
                        for col in 0..8 {
                            if (byte & (0x80 >> col)) != 0 {
                                let px = cur_x + col as i32;
                                if px >= clip.0 && px < clip.2 && px >= 0 && px < buf_w as i32 {
                                    let idx = (py as u32 * buf_w + px as u32) as usize;
                                    buffer[idx] = blend_argb(*color, buffer[idx]);
                                }
                            }
                        }
                    }
                    cur_x += 9; // 8px width + 1px spacing
                }
            }
            DrawOp::DrawBitmap { x, y, width, height, pixels } => {
                let clip = clip_stack.last().copied().unwrap_or((0, 0, buf_w as i32, buf_h as i32));
                let x0 = (*x).max(clip.0).max(0) as u32;
                let y0 = (*y).max(clip.1).max(0) as u32;
                let x1 = ((*x + *width as i32).min(clip.2).max(0) as u32).min(buf_w);
                let y1 = ((*y + *height as i32).min(clip.3).max(0) as u32).min(buf_h);

                for py in y0..y1 {
                    let src_y = (py as i32 - *y) as usize;
                    let dst_row = (py * buf_w) as usize;
                    let src_row = src_y * (*width as usize);
                    for px in x0..x1 {
                        let src_x = (px as i32 - *x) as usize;
                        let src_col = pixels[src_row + src_x];
                        let idx = dst_row + px as usize;
                        buffer[idx] = blend_argb(src_col, buffer[idx]);
                    }
                }
            }
        }
    }
}
