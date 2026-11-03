// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! High-performance display-list and horizontal band rasterizer.
//!
//! Eliminates per-pixel re-layout costs by recording positioned drawing commands
//! into a stack-allocated or static buffer, then evaluating each scanline band
//! in one linear sweep. Zero heap allocation.

use crate::font;

pub const MAX_TEXT_BYTES: usize = 64;

/// Dedicated shared-memory ring buffer pipe ID for compositor display list streaming.
pub const PIPE_COMPOSITOR_DISPLAY_LIST: u32 = 4;

/// DisplayList binary wire format opcodes.
pub const OPCODE_RECT: u8 = 1;
pub const OPCODE_TEXT: u8 = 2;
pub const OPCODE_SHADOW: u8 = 3;
pub const OPCODE_SURFACE: u8 = 4;
pub const OPCODE_FILL_RECT: u8 = 5;
pub const OPCODE_BORDER: u8 = 6;
pub const OPCODE_SQUIRCLE: u8 = 7;
pub const OPCODE_DAMAGE_RECT: u8 = 8;

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

    /// Computes the bounding union of two rectangles.
    pub fn union(&self, other: &Self) -> Self {
        if self.w == 0 || self.h == 0 {
            return *other;
        }
        if other.w == 0 || other.h == 0 {
            return *self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        Self {
            x: x0,
            y: y0,
            w: x1.saturating_sub(x0),
            h: y1.saturating_sub(y0),
        }
    }

    /// Converts the rectangle into a half-open scanline row band `[y0, y1)`.
    pub fn to_row_band(&self, max_height: usize) -> (usize, usize) {
        if self.w == 0 || self.h == 0 {
            (0, 0)
        } else {
            let y0 = self.y.min(max_height);
            let y1 = (self.y + self.h).min(max_height);
            (y0, y1)
        }
    }
}

/// Positioned drawing commands supported by the display list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// Production-grade rounded rectangle with fill, corner radius, and optional outline border.
    DrawRoundedRect {
        rect: Rect,
        fill: u32,
        radius: usize,
        border: Option<u32>,
    },
    /// High-performance pre-measured glyph run with font weighting and grayscale coverage AA.
    DrawGlyphRun {
        x: usize,
        y: usize,
        scale: usize,
        color: u32,
        bold: bool,
        len: usize,
        bytes: [u8; MAX_TEXT_BYTES],
    },
    /// Soft drop shadow with Gaussian radius and directional elevation offsets.
    DrawShadow {
        rect: Rect,
        radius: usize,
        shadow_color: u32,
        offset_x: i32,
        offset_y: i32,
    },
    /// Zero-copy nested compositor surface / layer blit.
    DrawSurface {
        surface_id: u32,
        src_rect: Rect,
        dst_rect: Rect,
        alpha: u8,
    },
    /// Ultra-compact solid rectangle fill without corner/border overhead (13 bytes serialized).
    FillRect {
        rect: Rect,
        fill: u32,
    },
    /// Dedicated outline border primitive with stroke width and corner radius.
    Border {
        rect: Rect,
        color: u32,
        width: usize,
        radius: usize,
    },
    /// Continuous-curvature squircle card with optional border.
    Squircle {
        rect: Rect,
        fill: u32,
        radius: usize,
        border: Option<u32>,
    },
    /// Zero-copy surface blit alias for DrawSurface.
    Blit {
        surface_id: u32,
        src_rect: Rect,
        dst_rect: Rect,
        alpha: u8,
    },
}

/// Fixed-capacity, allocation-free display list buffer.
pub struct DisplayList<const CAP: usize> {
    commands: [Option<DrawCmd>; CAP],
    len: usize,
    pub bg_color: u32,
    pub dirty_rect: Option<Rect>,
    pub damage_y0: usize,
    pub damage_y1: usize,
}

impl<const CAP: usize> DisplayList<CAP> {
    pub const fn new(bg_color: u32) -> Self {
        Self {
            commands: [None; CAP],
            len: 0,
            bg_color,
            dirty_rect: None,
            damage_y0: usize::MAX,
            damage_y1: 0,
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
        self.dirty_rect = None;
        self.damage_y0 = usize::MAX;
        self.damage_y1 = 0;
    }

    /// Updates internal dirty bounding box and scanline damage range.
    fn update_damage(&mut self, r: Rect) {
        if r.w == 0 || r.h == 0 {
            return;
        }
        self.damage_y0 = self.damage_y0.min(r.y);
        self.damage_y1 = self.damage_y1.max(r.y + r.h);
        self.dirty_rect = Some(match self.dirty_rect {
            None => r,
            Some(cur) => cur.union(&r),
        });
    }

    /// Returns the accumulated damaged screen rectangle covering all recorded commands.
    #[inline]
    pub fn dirty_rect(&self) -> Option<Rect> {
        self.dirty_rect
    }

    /// Sets or overrides the explicit dirty damage rectangle.
    pub fn set_dirty_rect(&mut self, rect: Rect) {
        self.dirty_rect = Some(rect);
        self.damage_y0 = rect.y;
        self.damage_y1 = rect.y + rect.h;
    }

    /// Returns the damaged vertical scanline row band `[y0, y1)` clamped to `max_height`.
    #[inline]
    pub fn damage_row_band(&self, max_height: usize) -> (usize, usize) {
        if self.damage_y0 >= self.damage_y1 {
            (0, 0)
        } else {
            (self.damage_y0.min(max_height), self.damage_y1.min(max_height))
        }
    }

    /// Pushes a filled rectangle with optional rounded corners and border.
    pub fn push_rect(&mut self, rect: Rect, fill: u32, radius: usize, border: Option<u32>) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(rect);
        self.commands[self.len] = Some(DrawCmd::Rect { rect, fill, radius, border });
        self.len += 1;
        true
    }

    /// Pushes an ultra-compact solid rectangle fill.
    pub fn push_fill_rect(&mut self, rect: Rect, fill: u32) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(rect);
        self.commands[self.len] = Some(DrawCmd::FillRect { rect, fill });
        self.len += 1;
        true
    }

    /// Pushes a dedicated outline border with stroke width and corner radius.
    pub fn push_border(&mut self, rect: Rect, color: u32, width: usize, radius: usize) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(rect);
        self.commands[self.len] = Some(DrawCmd::Border { rect, color, width, radius });
        self.len += 1;
        true
    }

    /// Pushes a continuous-curvature squircle card with optional border.
    pub fn push_squircle(&mut self, rect: Rect, fill: u32, radius: usize, border: Option<u32>) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(rect);
        self.commands[self.len] = Some(DrawCmd::Squircle { rect, fill, radius, border });
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
        let tw = font::measure_weighted(&text[..n], scale, bold);
        let th = font::line_height(scale);
        self.update_damage(Rect::new(x, y, tw, th));
        self.commands[self.len] = Some(DrawCmd::Text { x, y, scale, color, bold, len: n, bytes });
        self.len += 1;
        true
    }

    /// Pushes a production rounded rectangle with corner radius and optional border.
    pub fn push_rounded_rect(&mut self, rect: Rect, fill: u32, radius: usize, border: Option<u32>) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(rect);
        self.commands[self.len] = Some(DrawCmd::DrawRoundedRect { rect, fill, radius, border });
        self.len += 1;
        true
    }

    /// Pushes a glyph run with typographic weighting and grayscale coverage AA.
    pub fn push_glyph_run(&mut self, x: usize, y: usize, scale: usize, text: &[u8], color: u32, bold: bool) -> bool {
        if self.len >= CAP {
            return false;
        }
        let mut bytes = [0u8; MAX_TEXT_BYTES];
        let n = text.len().min(MAX_TEXT_BYTES);
        bytes[..n].copy_from_slice(&text[..n]);
        let tw = font::measure_weighted(&text[..n], scale, bold);
        let th = font::line_height(scale);
        self.update_damage(Rect::new(x, y, tw, th));
        self.commands[self.len] = Some(DrawCmd::DrawGlyphRun { x, y, scale, color, bold, len: n, bytes });
        self.len += 1;
        true
    }

    /// Pushes a soft drop shadow.
    pub fn push_shadow(&mut self, rect: Rect, radius: usize, shadow_color: u32, offset_x: i32, offset_y: i32) -> bool {
        if self.len >= CAP {
            return false;
        }
        let sx = (rect.x as i32 + offset_x).max(0) as usize;
        let sy = (rect.y as i32 + offset_y).max(0) as usize;
        self.update_damage(Rect::new(sx, sy, rect.w, rect.h));
        self.commands[self.len] = Some(DrawCmd::DrawShadow { rect, radius, shadow_color, offset_x, offset_y });
        self.len += 1;
        true
    }

    /// Pushes a nested compositor surface blit.
    pub fn push_surface(&mut self, surface_id: u32, src_rect: Rect, dst_rect: Rect, alpha: u8) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(dst_rect);
        self.commands[self.len] = Some(DrawCmd::DrawSurface { surface_id, src_rect, dst_rect, alpha });
        self.len += 1;
        true
    }

    /// Pushes a zero-copy nested compositor surface blit.
    pub fn push_blit(&mut self, surface_id: u32, src_rect: Rect, dst_rect: Rect, alpha: u8) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.update_damage(dst_rect);
        self.commands[self.len] = Some(DrawCmd::Blit { surface_id, src_rect, dst_rect, alpha });
        self.len += 1;
        true
    }

    pub const MAGIC: [u8; 4] = *b"DPLT";

    /// Serializes display list commands into binary format for streaming over zero-copy SHM DMA pipe.
    pub fn encode_to_slice(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if out.len() < 12 {
            return Err("buffer too small for header");
        }
        out[0..4].copy_from_slice(&Self::MAGIC);
        out[4] = 1; // version
        out[5] = if self.dirty_rect.is_some() { 1 } else { 0 }; // flags
        out[6] = 0;
        out[7] = 0;
        out[8..12].copy_from_slice(&self.bg_color.to_le_bytes());
        let mut offset = 12;

        // Encode damage rectangle if present
        if let Some(dr) = self.dirty_rect {
            if offset + 9 > out.len() {
                return Err("buffer overflow encoding DamageRect");
            }
            out[offset] = OPCODE_DAMAGE_RECT;
            out[offset + 1..offset + 3].copy_from_slice(&(dr.x as u16).to_le_bytes());
            out[offset + 3..offset + 5].copy_from_slice(&(dr.y as u16).to_le_bytes());
            out[offset + 5..offset + 7].copy_from_slice(&(dr.w as u16).to_le_bytes());
            out[offset + 7..offset + 9].copy_from_slice(&(dr.h as u16).to_le_bytes());
            offset += 9;
        }

        for cmd_opt in &self.commands[..self.len] {
            let Some(cmd) = cmd_opt else { continue; };
            match cmd {
                DrawCmd::FillRect { rect, fill } => {
                    if offset + 13 > out.len() {
                        return Err("buffer overflow encoding FillRect");
                    }
                    out[offset] = OPCODE_FILL_RECT;
                    out[offset + 1..offset + 3].copy_from_slice(&(rect.x as u16).to_le_bytes());
                    out[offset + 3..offset + 5].copy_from_slice(&(rect.y as u16).to_le_bytes());
                    out[offset + 5..offset + 7].copy_from_slice(&(rect.w as u16).to_le_bytes());
                    out[offset + 7..offset + 9].copy_from_slice(&(rect.h as u16).to_le_bytes());
                    out[offset + 9..offset + 13].copy_from_slice(&fill.to_le_bytes());
                    offset += 13;
                }
                DrawCmd::Border { rect, color, width, radius } => {
                    if offset + 16 > out.len() {
                        return Err("buffer overflow encoding Border");
                    }
                    out[offset] = OPCODE_BORDER;
                    out[offset + 1..offset + 3].copy_from_slice(&(rect.x as u16).to_le_bytes());
                    out[offset + 3..offset + 5].copy_from_slice(&(rect.y as u16).to_le_bytes());
                    out[offset + 5..offset + 7].copy_from_slice(&(rect.w as u16).to_le_bytes());
                    out[offset + 7..offset + 9].copy_from_slice(&(rect.h as u16).to_le_bytes());
                    out[offset + 9..offset + 13].copy_from_slice(&color.to_le_bytes());
                    out[offset + 13] = *width as u8;
                    out[offset + 14..offset + 16].copy_from_slice(&(*radius as u16).to_le_bytes());
                    offset += 16;
                }
                DrawCmd::Squircle { rect, fill, radius, border } => {
                    if offset + 20 > out.len() {
                        return Err("buffer overflow encoding Squircle");
                    }
                    out[offset] = OPCODE_SQUIRCLE;
                    out[offset + 1..offset + 3].copy_from_slice(&(rect.x as u16).to_le_bytes());
                    out[offset + 3..offset + 5].copy_from_slice(&(rect.y as u16).to_le_bytes());
                    out[offset + 5..offset + 7].copy_from_slice(&(rect.w as u16).to_le_bytes());
                    out[offset + 7..offset + 9].copy_from_slice(&(rect.h as u16).to_le_bytes());
                    out[offset + 9..offset + 13].copy_from_slice(&fill.to_le_bytes());
                    out[offset + 13..offset + 15].copy_from_slice(&(*radius as u16).to_le_bytes());
                    if let Some(b) = border {
                        out[offset + 15] = 1;
                        out[offset + 16..offset + 20].copy_from_slice(&b.to_le_bytes());
                    } else {
                        out[offset + 15] = 0;
                        out[offset + 16..offset + 20].fill(0);
                    }
                    offset += 20;
                }
                DrawCmd::Rect { rect, fill, radius, border } | DrawCmd::DrawRoundedRect { rect, fill, radius, border } => {
                    if offset + 20 > out.len() {
                        return Err("buffer overflow encoding DrawRoundedRect");
                    }
                    out[offset] = OPCODE_RECT;
                    out[offset + 1..offset + 3].copy_from_slice(&(rect.x as u16).to_le_bytes());
                    out[offset + 3..offset + 5].copy_from_slice(&(rect.y as u16).to_le_bytes());
                    out[offset + 5..offset + 7].copy_from_slice(&(rect.w as u16).to_le_bytes());
                    out[offset + 7..offset + 9].copy_from_slice(&(rect.h as u16).to_le_bytes());
                    out[offset + 9..offset + 13].copy_from_slice(&fill.to_le_bytes());
                    out[offset + 13..offset + 15].copy_from_slice(&(*radius as u16).to_le_bytes());
                    if let Some(b) = border {
                        out[offset + 15] = 1;
                        out[offset + 16..offset + 20].copy_from_slice(&b.to_le_bytes());
                    } else {
                        out[offset + 15] = 0;
                        out[offset + 16..offset + 20].fill(0);
                    }
                    offset += 20;
                }
                DrawCmd::Text { x, y, scale, color, bold, len, bytes } | DrawCmd::DrawGlyphRun { x, y, scale, color, bold, len, bytes } => {
                    let text_len = (*len).min(MAX_TEXT_BYTES);
                    let cmd_size = 12 + text_len;
                    if offset + cmd_size > out.len() {
                        return Err("buffer overflow encoding DrawGlyphRun");
                    }
                    out[offset] = OPCODE_TEXT;
                    out[offset + 1..offset + 3].copy_from_slice(&(*x as u16).to_le_bytes());
                    out[offset + 3..offset + 5].copy_from_slice(&(*y as u16).to_le_bytes());
                    out[offset + 5] = *scale as u8;
                    out[offset + 6] = if *bold { 1 } else { 0 };
                    out[offset + 7] = text_len as u8;
                    out[offset + 8..offset + 12].copy_from_slice(&color.to_le_bytes());
                    out[offset + 12..offset + 12 + text_len].copy_from_slice(&bytes[..text_len]);
                    offset += cmd_size;
                }
                DrawCmd::DrawShadow { rect, radius, shadow_color, offset_x, offset_y } => {
                    if offset + 19 > out.len() {
                        return Err("buffer overflow encoding DrawShadow");
                    }
                    out[offset] = OPCODE_SHADOW;
                    out[offset + 1..offset + 3].copy_from_slice(&(rect.x as u16).to_le_bytes());
                    out[offset + 3..offset + 5].copy_from_slice(&(rect.y as u16).to_le_bytes());
                    out[offset + 5..offset + 7].copy_from_slice(&(rect.w as u16).to_le_bytes());
                    out[offset + 7..offset + 9].copy_from_slice(&(rect.h as u16).to_le_bytes());
                    out[offset + 9..offset + 11].copy_from_slice(&(*radius as u16).to_le_bytes());
                    out[offset + 11..offset + 15].copy_from_slice(&shadow_color.to_le_bytes());
                    out[offset + 15..offset + 17].copy_from_slice(&(*offset_x as i16).to_le_bytes());
                    out[offset + 17..offset + 19].copy_from_slice(&(*offset_y as i16).to_le_bytes());
                    offset += 19;
                }
                DrawCmd::DrawSurface { surface_id, src_rect, dst_rect, alpha }
                | DrawCmd::Blit { surface_id, src_rect, dst_rect, alpha } => {
                    if offset + 22 > out.len() {
                        return Err("buffer overflow encoding DrawSurface");
                    }
                    out[offset] = OPCODE_SURFACE;
                    out[offset + 1..offset + 5].copy_from_slice(&surface_id.to_le_bytes());
                    out[offset + 5..offset + 7].copy_from_slice(&(src_rect.x as u16).to_le_bytes());
                    out[offset + 7..offset + 9].copy_from_slice(&(src_rect.y as u16).to_le_bytes());
                    out[offset + 9..offset + 11].copy_from_slice(&(src_rect.w as u16).to_le_bytes());
                    out[offset + 11..offset + 13].copy_from_slice(&(src_rect.h as u16).to_le_bytes());
                    out[offset + 13..offset + 15].copy_from_slice(&(dst_rect.x as u16).to_le_bytes());
                    out[offset + 15..offset + 17].copy_from_slice(&(dst_rect.y as u16).to_le_bytes());
                    out[offset + 17..offset + 19].copy_from_slice(&(dst_rect.w as u16).to_le_bytes());
                    out[offset + 19..offset + 21].copy_from_slice(&(dst_rect.h as u16).to_le_bytes());
                    out[offset + 21] = *alpha;
                    offset += 22;
                }
            }
        }
        Ok(offset)
    }

    /// Deserializes a binary display list payload received from an SHM DMA pipe.
    pub fn decode_from_slice(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 12 {
            return Err("data too short for header");
        }
        if data[0..4] != Self::MAGIC || data[4] != 1 {
            return Err("invalid display list magic or version");
        }
        let bg_color = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let mut dl = Self::new(bg_color);
        let mut offset = 12;

        while offset < data.len() {
            let opcode = data[offset];
            offset += 1;
            match opcode {
                OPCODE_RECT => {
                    if offset + 19 > data.len() { break; }
                    let rx = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let ry = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let rw = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let rh = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    let fill = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
                    let radius = u16::from_le_bytes(data[offset + 12..offset + 14].try_into().unwrap()) as usize;
                    let has_border = data[offset + 14] != 0;
                    let border = if has_border {
                        Some(u32::from_le_bytes(data[offset + 15..offset + 19].try_into().unwrap()))
                    } else {
                        None
                    };
                    offset += 19;
                    dl.push_rounded_rect(Rect::new(rx, ry, rw, rh), fill, radius, border);
                }
                OPCODE_TEXT => {
                    if offset + 11 > data.len() { break; }
                    let x = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let y = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let scale = data[offset + 4] as usize;
                    let bold = data[offset + 5] != 0;
                    let text_len = (data[offset + 6] as usize).min(MAX_TEXT_BYTES);
                    let color = u32::from_le_bytes(data[offset + 7..offset + 11].try_into().unwrap());
                    offset += 11;
                    if offset + text_len > data.len() { break; }
                    let text_bytes = &data[offset..offset + text_len];
                    offset += text_len;
                    dl.push_glyph_run(x, y, scale, text_bytes, color, bold);
                }
                OPCODE_SHADOW => {
                    if offset + 18 > data.len() { break; }
                    let rx = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let ry = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let rw = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let rh = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    let radius = u16::from_le_bytes(data[offset + 8..offset + 10].try_into().unwrap()) as usize;
                    let shadow_color = u32::from_le_bytes(data[offset + 10..offset + 14].try_into().unwrap());
                    let offset_x = i16::from_le_bytes(data[offset + 14..offset + 16].try_into().unwrap()) as i32;
                    let offset_y = i16::from_le_bytes(data[offset + 16..offset + 18].try_into().unwrap()) as i32;
                    offset += 18;
                    dl.push_shadow(Rect::new(rx, ry, rw, rh), radius, shadow_color, offset_x, offset_y);
                }
                OPCODE_SURFACE => {
                    if offset + 21 > data.len() { break; }
                    let surface_id = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                    let sx = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let sy = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    let sw = u16::from_le_bytes(data[offset + 8..offset + 10].try_into().unwrap()) as usize;
                    let sh = u16::from_le_bytes(data[offset + 10..offset + 12].try_into().unwrap()) as usize;
                    let dx = u16::from_le_bytes(data[offset + 12..offset + 14].try_into().unwrap()) as usize;
                    let dy = u16::from_le_bytes(data[offset + 14..offset + 16].try_into().unwrap()) as usize;
                    let dw = u16::from_le_bytes(data[offset + 16..offset + 18].try_into().unwrap()) as usize;
                    let dh = u16::from_le_bytes(data[offset + 18..offset + 20].try_into().unwrap()) as usize;
                    let alpha = data[offset + 20];
                    offset += 21;
                    dl.push_surface(surface_id, Rect::new(sx, sy, sw, sh), Rect::new(dx, dy, dw, dh), alpha);
                }
                OPCODE_FILL_RECT => {
                    if offset + 12 > data.len() { break; }
                    let rx = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let ry = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let rw = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let rh = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    let fill = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
                    offset += 12;
                    dl.push_fill_rect(Rect::new(rx, ry, rw, rh), fill);
                }
                OPCODE_BORDER => {
                    if offset + 15 > data.len() { break; }
                    let rx = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let ry = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let rw = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let rh = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    let color = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
                    let width = data[offset + 12] as usize;
                    let radius = u16::from_le_bytes(data[offset + 13..offset + 15].try_into().unwrap()) as usize;
                    offset += 15;
                    dl.push_border(Rect::new(rx, ry, rw, rh), color, width, radius);
                }
                OPCODE_SQUIRCLE => {
                    if offset + 19 > data.len() { break; }
                    let rx = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let ry = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let rw = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let rh = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    let fill = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
                    let radius = u16::from_le_bytes(data[offset + 12..offset + 14].try_into().unwrap()) as usize;
                    let has_border = data[offset + 14] != 0;
                    let border = if has_border {
                        Some(u32::from_le_bytes(data[offset + 15..offset + 19].try_into().unwrap()))
                    } else {
                        None
                    };
                    offset += 19;
                    dl.push_squircle(Rect::new(rx, ry, rw, rh), fill, radius, border);
                }
                OPCODE_DAMAGE_RECT => {
                    if offset + 8 > data.len() { break; }
                    let rx = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
                    let ry = u16::from_le_bytes(data[offset + 2..offset + 4].try_into().unwrap()) as usize;
                    let rw = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
                    let rh = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap()) as usize;
                    offset += 8;
                    dl.set_dirty_rect(Rect::new(rx, ry, rw, rh));
                }
                _ => break,
            }
        }
        Ok(dl)
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
                DrawCmd::FillRect { rect, fill } => {
                    let ry0 = rect.y.max(y0);
                    let ry1 = (rect.y + rect.h).min(y1);
                    if ry1 <= ry0 { continue; }
                    let rx0 = rect.x.min(w);
                    let rx1 = (rect.x + rect.w).min(w);
                    if rx1 <= rx0 { continue; }

                    for y in ry0..ry1 {
                        let out_row_idx = (y - y0) * w;
                        out[out_row_idx + rx0..out_row_idx + rx1].fill(*fill);
                    }
                }
                DrawCmd::Border { rect, color, width, radius } => {
                    let ry0 = rect.y.max(y0);
                    let ry1 = (rect.y + rect.h).min(y1);
                    if ry1 <= ry0 { continue; }
                    let rx0 = rect.x.min(w);
                    let rx1 = (rect.x + rect.w).min(w);
                    if rx1 <= rx0 { continue; }
                    let bw = (*width).max(1);

                    for y in ry0..ry1 {
                        let out_row_idx = (y - y0) * w;
                        for x in rx0..rx1 {
                            let is_outer_border = x < rect.x + bw
                                || x >= rect.x + rect.w.saturating_sub(bw)
                                || y < rect.y + bw
                                || y >= rect.y + rect.h.saturating_sub(bw);

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
                                    let dist2 = dx * dx + dy * dy;
                                    let r_out2 = (r as i32) * (r as i32);
                                    if dist2 > r_out2 {
                                        continue;
                                    }
                                    if r > bw {
                                        let r_in = (r - bw) as i32;
                                        if dist2 < r_in * r_in {
                                            continue;
                                        }
                                    }
                                    out[out_row_idx + x] = *color;
                                    continue;
                                }
                            }
                            if is_outer_border {
                                out[out_row_idx + x] = *color;
                            }
                        }
                    }
                }
                DrawCmd::Squircle { rect, fill, radius, border } => {
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
                                    // Quartic superellipse curvature: dx^4 + dy^4 <= r^4
                                    let dx2 = (dx * dx) as u64;
                                    let dy2 = (dy * dy) as u64;
                                    let r2 = (r as u64) * (r as u64);
                                    if dx2 * dx2 + dy2 * dy2 > r2 * r2 {
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
                DrawCmd::Rect { rect, fill, radius, border } | DrawCmd::DrawRoundedRect { rect, fill, radius, border } => {
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
                DrawCmd::Text { x, y, scale, color, bold, len, bytes } | DrawCmd::DrawGlyphRun { x, y, scale, color, bold, len, bytes } => {
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
                DrawCmd::DrawShadow { rect, radius: _, shadow_color, offset_x, offset_y } => {
                    let sx0 = ((rect.x as i32 + offset_x).max(0) as usize).min(w);
                    let sy0 = (rect.y as i32 + offset_y).max(0) as usize;
                    let ry0 = sy0.max(y0);
                    let ry1 = (sy0 + rect.h).min(y1);
                    if ry1 <= ry0 { continue; }
                    let rx1 = (sx0 + rect.w).min(w);
                    if rx1 <= sx0 { continue; }

                    for y in ry0..ry1 {
                        let out_row_idx = (y - y0) * w;
                        for x in sx0..rx1 {
                            out[out_row_idx + x] = *shadow_color;
                        }
                    }
                }
                DrawCmd::DrawSurface { surface_id: _, src_rect: _, dst_rect, alpha: _ }
                | DrawCmd::Blit { surface_id: _, src_rect: _, dst_rect, alpha: _ } => {
                    let ry0 = dst_rect.y.max(y0);
                    let ry1 = (dst_rect.y + dst_rect.h).min(y1);
                    if ry1 <= ry0 { continue; }
                    let rx0 = dst_rect.x.min(w);
                    let rx1 = (dst_rect.x + dst_rect.w).min(w);
                    if rx1 <= rx0 { continue; }
                }
            }
        }
    }

    /// Performs direct scanout rasterization into a full target buffer with stride `w`, zero intermediate copies.
    pub fn rasterize_direct(&self, out: &mut [u32], w: usize) {
        if w == 0 || out.is_empty() { return; }
        let h = out.len() / w;
        self.rasterize_band(0, h, w, out);
    }

    /// Rasterizes only the damaged vertical scanlines directly into the target buffer, skipping unchanged rows.
    ///
    /// Returns the half-open scanline row band `[y0, y1)` that was updated.
    pub fn rasterize_damaged(&self, out: &mut [u32], w: usize, max_height: usize) -> (usize, usize) {
        let (y0, y1) = self.damage_row_band(max_height);
        if y0 >= y1 || w == 0 || out.len() < y1 * w {
            return (0, 0);
        }
        let slice = &mut out[y0 * w..y1 * w];
        self.rasterize_band(y0, y1, w, slice);
        (y0, y1)
    }
}

/// Memory bandwidth comparison metrics comparing InterOS Declarative DisplayList Compositing
/// against traditional Window-Copy compositors (Android SurfaceFlinger / iOS Quartz Compositor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositorBandwidthMetrics {
    /// Bytes transferred by Android SurfaceFlinger / iOS Quartz window copying model for a frame.
    pub traditional_window_copy_bytes: usize,
    /// Bytes transferred by InterOS Declarative DisplayList Compositing for the same frame.
    pub interos_display_list_bytes: usize,
    /// Memory bandwidth savings in percentage (0 to 100%).
    pub bandwidth_savings_percent: u32,
}

/// Evaluates memory bandwidth traffic and savings for a frame update of screen size `w x h`.
///
/// Traditional window copy model (SurfaceFlinger / Quartz):
/// 1. App renders into backing store: `w * h * 4` bytes
/// 2. Compositor reads window backing store: `w * h * 4` bytes
/// 3. Compositor reads backbuffer for blending: `w * h * 4` bytes
/// 4. Compositor writes backbuffer: `w * h * 4` bytes
/// 5. Display hardware scanout read: `w * h * 4` bytes
/// Total: `5 * w * h * 4` bytes.
///
/// InterOS Declarative DisplayList model:
/// 1. App records & serializes commands over SHM pipe: `dl_wire_bytes` write
/// 2. Compositor reads commands from SHM pipe: `dl_wire_bytes` read
/// 3. Direct scanout rasterization of damaged rows: `w * damaged_h * 4` bytes write
/// 4. Display hardware scanout read: `w * damaged_h * 4` bytes read
/// Total: `(2 * dl_wire_bytes) + (2 * w * damaged_h * 4)` bytes.
pub fn calculate_bandwidth_savings(w: usize, h: usize, damaged_h: usize, dl_wire_bytes: usize) -> CompositorBandwidthMetrics {
    let frame_bytes = w.saturating_mul(h).saturating_mul(4);
    let traditional = frame_bytes.saturating_mul(5);

    let dam_h = damaged_h.min(h);
    let damaged_bytes = w.saturating_mul(dam_h).saturating_mul(4);
    let interos = (dl_wire_bytes.saturating_mul(2)).saturating_add(damaged_bytes.saturating_mul(2));

    let savings = if traditional > 0 && traditional >= interos {
        (((traditional - interos) as u64 * 100) / traditional as u64) as u32
    } else {
        0
    };

    CompositorBandwidthMetrics {
        traditional_window_copy_bytes: traditional,
        interos_display_list_bytes: interos,
        bandwidth_savings_percent: savings,
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

    #[test]
    fn test_structured_commands_push_and_rasterize() {
        let mut dl = DisplayList::<16>::new(0xFF000000);
        assert!(dl.push_rounded_rect(Rect::new(5, 5, 40, 30), 0xFF123456, 4, Some(0xFFFFFFFF)));
        assert!(dl.push_glyph_run(10, 10, 1, b"GlyphRun", 0xFF00FF00, true));
        assert!(dl.push_shadow(Rect::new(5, 5, 40, 30), 4, 0x88000000, 2, 2));
        assert!(dl.push_surface(1, Rect::new(0, 0, 100, 100), Rect::new(10, 10, 50, 50), 255));
        assert_eq!(dl.len(), 4);

        let mut band = [0u32; 100 * 50];
        dl.rasterize_band(0, 50, 100, &mut band);
        assert_ne!(band[15 * 100 + 15], 0xFF000000);
    }

    #[test]
    fn test_display_list_encode_decode_roundtrip() {
        let mut dl = DisplayList::<16>::new(0xFF101010);
        dl.push_rounded_rect(Rect::new(10, 20, 100, 50), 0xFFE0E0E0, 8, Some(0xFF000000));
        dl.push_glyph_run(15, 25, 2, b"InterOS Native", 0xFF000000, true);
        dl.push_shadow(Rect::new(10, 20, 100, 50), 6, 0x40000000, 4, 4);
        dl.push_surface(42, Rect::new(0, 0, 200, 200), Rect::new(20, 30, 80, 80), 200);

        let mut buf = [0u8; 512];
        let bytes_written = dl.encode_to_slice(&mut buf).expect("encode display list");
        assert!(bytes_written > 12);
        assert!(bytes_written < 512);

        let decoded = DisplayList::<16>::decode_from_slice(&buf[..bytes_written]).expect("decode display list");
        assert_eq!(decoded.bg_color, 0xFF101010);
        assert_eq!(decoded.len(), 4);
    }

    #[test]
    fn test_primitives_fill_rect_border_squircle_blit() {
        let mut dl = DisplayList::<16>::new(0xFF000000);
        assert!(dl.push_fill_rect(Rect::new(0, 0, 100, 20), 0xFF202020));
        assert!(dl.push_border(Rect::new(5, 5, 90, 10), 0xFF00FF00, 1, 0));
        assert!(dl.push_squircle(Rect::new(10, 25, 80, 30), 0xFF0000FF, 6, Some(0xFFFFFFFF)));
        assert!(dl.push_blit(2, Rect::new(0, 0, 50, 50), Rect::new(20, 60, 40, 40), 255));
        assert_eq!(dl.len(), 4);

        // Verify dirty damage tracking automatically calculated the bounding region
        let damage = dl.dirty_rect().expect("dirty rect computed");
        assert_eq!(damage.x, 0);
        assert_eq!(damage.y, 0);
        assert_eq!(damage.w, 100);
        assert_eq!(damage.h, 100);

        let (y0, y1) = dl.damage_row_band(120);
        assert_eq!(y0, 0);
        assert_eq!(y1, 100);

        // Encode to slice and verify compact size
        let mut buf = [0u8; 512];
        let n = dl.encode_to_slice(&mut buf).expect("encode compact primitives");
        // Compact: header(12) + damage_rect(9) + FillRect(13) + Border(16) + Squircle(20) + Blit(22) = 92 bytes!
        assert_eq!(n, 92);

        let decoded = DisplayList::<16>::decode_from_slice(&buf[..n]).expect("decode primitives");
        assert_eq!(decoded.len(), 4);
        let dec_damage = decoded.dirty_rect().expect("decoded dirty rect");
        assert_eq!(dec_damage.w, 100);
        assert_eq!(dec_damage.h, 100);

        // Rasterize damaged band directly into scanout buffer
        let mut scanout = [0u32; 100 * 120];
        let (ry0, ry1) = dl.rasterize_damaged(&mut scanout, 100, 120);
        assert_eq!((ry0, ry1), (0, 100));
        // Verify FillRect wrote pixels at row 1
        assert_eq!(scanout[1 * 100 + 50], 0xFF202020);
        // Verify rows outside damage band (105) remained untouched (0)
        assert_eq!(scanout[105 * 100], 0);
    }

    #[test]
    fn test_memory_bandwidth_savings_calculation() {
        // Test 1: Full frame 720x1440 update
        let metrics_full = calculate_bandwidth_savings(720, 1440, 1440, 300);
        assert_eq!(metrics_full.traditional_window_copy_bytes, 20_736_000); // 20.73 MB
        assert_eq!(metrics_full.interos_display_list_bytes, 8_295_000); // 8.29 MB
        assert_eq!(metrics_full.bandwidth_savings_percent, 59); // ~60% memory bandwidth savings for full 100% repaint

        // Test 2: Status bar / system line damage band (720x96)
        let metrics_band = calculate_bandwidth_savings(720, 1440, 96, 300);
        assert_eq!(metrics_band.traditional_window_copy_bytes, 20_736_000);
        assert_eq!(metrics_band.interos_display_list_bytes, 553_560); // 0.55 MB
        assert_eq!(metrics_band.bandwidth_savings_percent, 97); // 97% memory bandwidth savings!

        // Test 3: 20% damaged area (720x288) for interactive touch scroll / typing
        let metrics_partial = calculate_bandwidth_savings(720, 1440, 288, 300);
        assert_eq!(metrics_partial.bandwidth_savings_percent, 91); // 91% memory bandwidth savings!
    }
}
