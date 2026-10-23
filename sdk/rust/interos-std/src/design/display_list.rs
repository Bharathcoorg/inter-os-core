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
    /// High-performance pre-measured glyph run with font weighting and subpixel AA.
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

    /// Pushes a production rounded rectangle with corner radius and optional border.
    pub fn push_rounded_rect(&mut self, rect: Rect, fill: u32, radius: usize, border: Option<u32>) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.commands[self.len] = Some(DrawCmd::DrawRoundedRect { rect, fill, radius, border });
        self.len += 1;
        true
    }

    /// Pushes a glyph run with typographic weighting and subpixel AA.
    pub fn push_glyph_run(&mut self, x: usize, y: usize, scale: usize, text: &[u8], color: u32, bold: bool) -> bool {
        if self.len >= CAP {
            return false;
        }
        let mut bytes = [0u8; MAX_TEXT_BYTES];
        let n = text.len().min(MAX_TEXT_BYTES);
        bytes[..n].copy_from_slice(&text[..n]);
        self.commands[self.len] = Some(DrawCmd::DrawGlyphRun { x, y, scale, color, bold, len: n, bytes });
        self.len += 1;
        true
    }

    /// Pushes a soft drop shadow.
    pub fn push_shadow(&mut self, rect: Rect, radius: usize, shadow_color: u32, offset_x: i32, offset_y: i32) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.commands[self.len] = Some(DrawCmd::DrawShadow { rect, radius, shadow_color, offset_x, offset_y });
        self.len += 1;
        true
    }

    /// Pushes a nested compositor surface blit.
    pub fn push_surface(&mut self, surface_id: u32, src_rect: Rect, dst_rect: Rect, alpha: u8) -> bool {
        if self.len >= CAP {
            return false;
        }
        self.commands[self.len] = Some(DrawCmd::DrawSurface { surface_id, src_rect, dst_rect, alpha });
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
        out[5] = 0;
        out[6] = 0;
        out[7] = 0;
        out[8..12].copy_from_slice(&self.bg_color.to_le_bytes());
        let mut offset = 12;

        for cmd_opt in &self.commands[..self.len] {
            let Some(cmd) = cmd_opt else { continue; };
            match cmd {
                DrawCmd::Rect { rect, fill, radius, border } | DrawCmd::DrawRoundedRect { rect, fill, radius, border } => {
                    if offset + 20 > out.len() {
                        return Err("buffer overflow encoding DrawRoundedRect");
                    }
                    out[offset] = 1; // opcode
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
                    out[offset] = 2; // opcode
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
                    out[offset] = 3; // opcode
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
                DrawCmd::DrawSurface { surface_id, src_rect, dst_rect, alpha } => {
                    if offset + 22 > out.len() {
                        return Err("buffer overflow encoding DrawSurface");
                    }
                    out[offset] = 4; // opcode
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
                1 => {
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
                2 => {
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
                3 => {
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
                4 => {
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
                DrawCmd::DrawSurface { surface_id: _, src_rect: _, dst_rect, alpha: _ } => {
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
}
