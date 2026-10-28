//! Bounded SVG subset renderer for InterOS Core.
//!
//! # Platform Guarantees
//! - `#![no_std]` and zero heap allocation in render paths.
//! - Fixed-point 16.16 arithmetic (no floating point instructions).
//! - 16-subscanline coverage-AA rasterizer with non-zero winding rule.
//! - Porter-Duff Over alpha compositing into RGBA8 destination buffer.
//! - Bounded memory usage (stack / static buffers up to `MAX_SEGMENTS`).

pub const MAX_SEGMENTS: usize = 1024;
pub const MAX_CROSSINGS: usize = 1024;
pub const MAX_WIDTH: usize = 1280;
pub const SUBSAMPLES: i32 = 16;
pub const ONE: i32 = 1 << 16;
pub const FULL_COVER: i32 = 256;

/// Kappa constant for cubic Bézier circle approximation: 0.55228475 * 65536
pub const KAPPA: i32 = 36196;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    None,
    Rgba(u8, u8, u8, u8),
}

/// Helper to scan numbers from byte streams without allocation.
pub struct NumberScanner<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> NumberScanner<'a> {
    #[inline]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn skip_whitespace_and_commas(&mut self) {
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b',' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    pub fn next_fixed(&mut self) -> Option<i32> {
        self.skip_whitespace_and_commas();
        if self.pos >= self.data.len() {
            return None;
        }

        let negative = if self.data[self.pos] == b'-' {
            self.pos += 1;
            true
        } else if self.data[self.pos] == b'+' {
            self.pos += 1;
            false
        } else {
            false
        };

        let mut int_val = 0i64;
        let mut has_digits = false;
        while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
            int_val = int_val.saturating_mul(10).saturating_add((self.data[self.pos] - b'0') as i64);
            has_digits = true;
            self.pos += 1;
        }

        let mut frac_16 = 0i64;
        if self.pos < self.data.len() && self.data[self.pos] == b'.' {
            self.pos += 1;
            let mut frac_num = 0i64;
            let mut frac_den = 1i64;
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                has_digits = true;
                if frac_den < 10_000_000 {
                    frac_num = frac_num * 10 + (self.data[self.pos] - b'0') as i64;
                    frac_den *= 10;
                }
                self.pos += 1;
            }
            if frac_den > 1 {
                frac_16 = (frac_num * 65536) / frac_den;
            }
        }

        if !has_digits {
            return None;
        }

        let mut total = (int_val << 16) + frac_16;

        // Handle optional exponent (e.g. 1e2 or 1e-2)
        if self.pos < self.data.len() && (self.data[self.pos] == b'e' || self.data[self.pos] == b'E') {
            self.pos += 1;
            let exp_neg = if self.pos < self.data.len() && self.data[self.pos] == b'-' {
                self.pos += 1;
                true
            } else if self.pos < self.data.len() && self.data[self.pos] == b'+' {
                self.pos += 1;
                false
            } else {
                false
            };
            let mut exp_val = 0i32;
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                exp_val = exp_val * 10 + (self.data[self.pos] - b'0') as i32;
                self.pos += 1;
            }
            let exp_val = exp_val.min(6);
            if exp_neg {
                for _ in 0..exp_val {
                    total /= 10;
                }
            } else {
                for _ in 0..exp_val {
                    total *= 10;
                }
            }
        }

        // Skip unit identifiers if any (e.g. "px", "pt")
        if self.pos + 1 < self.data.len() {
            let u0 = self.data[self.pos];
            let u1 = self.data[self.pos + 1];
            if (u0 == b'p' || u0 == b'P') && (u1 == b'x' || u1 == b'X' || u1 == b't' || u1 == b'T') {
                self.pos += 2;
            }
        }

        let res = if negative { -total } else { total };
        Some(res as i32)
    }
}

pub fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn get_attr<'a>(tag: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i < tag.len() {
        if tag[i..].starts_with(key) {
            let valid_start = i == 0 || tag[i - 1].is_ascii_whitespace();
            let after_key = i + key.len();
            if valid_start && after_key < tag.len() {
                let mut j = after_key;
                while j < tag.len() && tag[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < tag.len() && tag[j] == b'=' {
                    j += 1;
                    while j < tag.len() && tag[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < tag.len() {
                        let quote = tag[j];
                        if quote == b'"' || quote == b'\'' {
                            j += 1;
                            let val_start = j;
                            while j < tag.len() && tag[j] != quote {
                                j += 1;
                            }
                            if j <= tag.len() {
                                return Some(&tag[val_start..j]);
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

pub fn get_style_prop<'a>(style: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i < style.len() {
        if style[i..].starts_with(key) {
            let valid_start = i == 0 || style[i - 1].is_ascii_whitespace() || style[i - 1] == b';';
            let after_key = i + key.len();
            if valid_start && after_key < style.len() {
                let mut j = after_key;
                while j < style.len() && style[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < style.len() && style[j] == b':' {
                    j += 1;
                    while j < style.len() && style[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    let val_start = j;
                    while j < style.len() && style[j] != b';' {
                        j += 1;
                    }
                    let mut val_end = j;
                    while val_end > val_start && style[val_end - 1].is_ascii_whitespace() {
                        val_end -= 1;
                    }
                    return Some(&style[val_start..val_end]);
                }
            }
        }
        i += 1;
    }
    None
}

pub fn get_attr_or_style<'a>(tag: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    if let Some(val) = get_attr(tag, key) {
        return Some(val);
    }
    if let Some(style) = get_attr(tag, b"style") {
        return get_style_prop(style, key);
    }
    None
}

pub fn parse_color(val: &[u8]) -> Option<Color> {
    let mut s = val;
    while !s.is_empty() && s[0].is_ascii_whitespace() {
        s = &s[1..];
    }
    while !s.is_empty() && s[s.len() - 1].is_ascii_whitespace() {
        s = &s[..s.len() - 1];
    }
    if s.is_empty() {
        return None;
    }
    if s.eq_ignore_ascii_case(b"none") {
        return Some(Color::None);
    }
    if s.eq_ignore_ascii_case(b"currentcolor") {
        return Some(Color::Rgba(0, 0, 0, 255));
    }
    if s.starts_with(b"#") {
        let hex = &s[1..];
        if hex.len() == 3 {
            let r = hex_digit(hex[0])?;
            let g = hex_digit(hex[1])?;
            let b = hex_digit(hex[2])?;
            return Some(Color::Rgba((r << 4) | r, (g << 4) | g, (b << 4) | b, 255));
        } else if hex.len() == 6 {
            let r = (hex_digit(hex[0])? << 4) | hex_digit(hex[1])?;
            let g = (hex_digit(hex[2])? << 4) | hex_digit(hex[3])?;
            let b = (hex_digit(hex[4])? << 4) | hex_digit(hex[5])?;
            return Some(Color::Rgba(r, g, b, 255));
        } else if hex.len() == 8 {
            let r = (hex_digit(hex[0])? << 4) | hex_digit(hex[1])?;
            let g = (hex_digit(hex[2])? << 4) | hex_digit(hex[3])?;
            let b = (hex_digit(hex[4])? << 4) | hex_digit(hex[5])?;
            let a = (hex_digit(hex[6])? << 4) | hex_digit(hex[7])?;
            return Some(Color::Rgba(r, g, b, a));
        }
        return None;
    }
    if s.eq_ignore_ascii_case(b"black") {
        return Some(Color::Rgba(0, 0, 0, 255));
    }
    if s.eq_ignore_ascii_case(b"white") {
        return Some(Color::Rgba(255, 255, 255, 255));
    }
    if s.eq_ignore_ascii_case(b"red") {
        return Some(Color::Rgba(255, 0, 0, 255));
    }
    if s.eq_ignore_ascii_case(b"green") {
        return Some(Color::Rgba(0, 128, 0, 255));
    }
    if s.eq_ignore_ascii_case(b"lime") {
        return Some(Color::Rgba(0, 255, 0, 255));
    }
    if s.eq_ignore_ascii_case(b"blue") {
        return Some(Color::Rgba(0, 0, 255, 255));
    }
    if s.eq_ignore_ascii_case(b"yellow") {
        return Some(Color::Rgba(255, 255, 0, 255));
    }
    if s.eq_ignore_ascii_case(b"cyan") {
        return Some(Color::Rgba(0, 255, 255, 255));
    }
    if s.eq_ignore_ascii_case(b"magenta") {
        return Some(Color::Rgba(255, 0, 255, 255));
    }
    if s.eq_ignore_ascii_case(b"gray") || s.eq_ignore_ascii_case(b"grey") {
        return Some(Color::Rgba(128, 128, 128, 255));
    }
    if s.eq_ignore_ascii_case(b"lightgray") || s.eq_ignore_ascii_case(b"lightgrey") {
        return Some(Color::Rgba(211, 211, 211, 255));
    }
    if s.eq_ignore_ascii_case(b"darkgray") || s.eq_ignore_ascii_case(b"darkgrey") {
        return Some(Color::Rgba(169, 169, 169, 255));
    }
    None
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn parse_fixed(val: &[u8]) -> Option<i32> {
    let mut scanner = NumberScanner::new(val);
    scanner.next_fixed()
}

pub fn parse_dimension(val: &[u8]) -> Option<u32> {
    let fixed = parse_fixed(val)?;
    if fixed <= 0 {
        return None;
    }
    Some(((fixed + 32768) >> 16) as u32)
}

pub fn parse_view_box(val: &[u8]) -> Option<(i32, i32, i32, i32)> {
    let mut scanner = NumberScanner::new(val);
    let x = scanner.next_fixed()?;
    let y = scanner.next_fixed()?;
    let w = scanner.next_fixed()?;
    let h = scanner.next_fixed()?;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((x, y, w, h))
}

#[inline]
pub fn isqrt64(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

pub struct SegmentBuffer {
    pub sx0: [i32; MAX_SEGMENTS],
    pub sy0: [i32; MAX_SEGMENTS],
    pub sx1: [i32; MAX_SEGMENTS],
    pub sy1: [i32; MAX_SEGMENTS],
    pub len: usize,
}

impl SegmentBuffer {
    pub fn new() -> Self {
        Self {
            sx0: [0; MAX_SEGMENTS],
            sy0: [0; MAX_SEGMENTS],
            sx1: [0; MAX_SEGMENTS],
            sy1: [0; MAX_SEGMENTS],
            len: 0,
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
    }

    #[inline]
    pub fn push(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) -> Result<(), ()> {
        if self.len >= MAX_SEGMENTS {
            return Err(());
        }
        self.sx0[self.len] = x0;
        self.sy0[self.len] = y0;
        self.sx1[self.len] = x1;
        self.sy1[self.len] = y1;
        self.len += 1;
        Ok(())
    }
}

pub fn flatten_cubic(
    p0: (i32, i32),
    p1: (i32, i32),
    p2: (i32, i32),
    p3: (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    let mut prev = p0;
    for i in 1..=8 {
        let u = (8 - i) as i64;
        let v = i as i64;
        let w0 = u * u * u;
        let w1 = 3 * u * u * v;
        let w2 = 3 * u * v * v;
        let w3 = v * v * v;
        let x = (w0 * p0.0 as i64 + w1 * p1.0 as i64 + w2 * p2.0 as i64 + w3 * p3.0 as i64 + 256) / 512;
        let y = (w0 * p0.1 as i64 + w1 * p1.1 as i64 + w2 * p2.1 as i64 + w3 * p3.1 as i64 + 256) / 512;
        let curr = (x as i32, y as i32);
        segments.push(prev.0, prev.1, curr.0, curr.1)?;
        prev = curr;
    }
    Ok(())
}

pub fn flatten_quad(
    p0: (i32, i32),
    p1: (i32, i32),
    p2: (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    let mut prev = p0;
    for i in 1..=8 {
        let u = (8 - i) as i64;
        let v = i as i64;
        let w0 = u * u;
        let w1 = 2 * u * v;
        let w2 = v * v;
        let x = (w0 * p0.0 as i64 + w1 * p1.0 as i64 + w2 * p2.0 as i64 + 32) / 64;
        let y = (w0 * p0.1 as i64 + w1 * p1.1 as i64 + w2 * p2.1 as i64 + 32) / 64;
        let curr = (x as i32, y as i32);
        segments.push(prev.0, prev.1, curr.0, curr.1)?;
        prev = curr;
    }
    Ok(())
}

pub struct SvgRasterizer {
    pub acc: [i32; MAX_WIDTH],
    pub cross_x: [i32; MAX_CROSSINGS],
    pub cross_dir: [i8; MAX_CROSSINGS],
}

impl SvgRasterizer {
    pub fn new() -> Self {
        Self {
            acc: [0; MAX_WIDTH],
            cross_x: [0; MAX_CROSSINGS],
            cross_dir: [0; MAX_CROSSINGS],
        }
    }

    pub fn rasterize(
        &mut self,
        segments: &SegmentBuffer,
        target_w: usize,
        target_h: usize,
        color: (u8, u8, u8, u8),
        out_rgba: &mut [u8],
    ) {
        if segments.len == 0 || target_w == 0 || target_h == 0 {
            return;
        }

        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for s in 0..segments.len {
            min_x = min_x.min(segments.sx0[s]).min(segments.sx1[s]);
            max_x = max_x.max(segments.sx0[s]).max(segments.sx1[s]);
            min_y = min_y.min(segments.sy0[s]).min(segments.sy1[s]);
            max_y = max_y.max(segments.sy0[s]).max(segments.sy1[s]);
        }

        let row_lo = min_y >> 16;
        let row_hi = (max_y >> 16) + 1;
        let col_lo = (min_x >> 16).max(0) as usize;
        let col_hi = (((max_x >> 16) + 1).max(0) as usize).min(target_w);
        let row_lo_us = (row_lo.max(0) as usize).min(target_h);
        let row_hi_us = (row_hi.max(0) as usize).min(target_h);

        if col_lo >= col_hi || row_lo_us >= row_hi_us {
            return;
        }

        let span = (col_hi - col_lo).min(MAX_WIDTH);
        let base = col_lo as i32 * ONE;

        for row in row_lo_us..row_hi_us {
            for v in self.acc[..span].iter_mut() {
                *v = 0;
            }
            for s in 0..SUBSAMPLES {
                let sy = (row as i32) * ONE + ((s * 2 + 1) * ONE) / (2 * SUBSAMPLES);
                let mut n_cross = 0usize;
                for k in 0..segments.len {
                    let (x0, y0, x1, y1) = (
                        segments.sx0[k],
                        segments.sy0[k],
                        segments.sx1[k],
                        segments.sy1[k],
                    );
                    if (y0 <= sy) == (y1 <= sy) {
                        continue;
                    }
                    let num = ((sy - y0) as i64) * ((x1 - x0) as i64);
                    let den = (y1 - y0) as i64;
                    let hit = x0 as i64 + num / den;
                    if n_cross < MAX_CROSSINGS {
                        self.cross_x[n_cross] = hit as i32 - base;
                        self.cross_dir[n_cross] = if y1 > y0 { 1 } else { -1 };
                        n_cross += 1;
                    }
                }
                if n_cross < 2 {
                    continue;
                }
                for a in 1..n_cross {
                    let mut b = a;
                    while b > 0 && self.cross_x[b - 1] > self.cross_x[b] {
                        self.cross_x.swap(b - 1, b);
                        self.cross_dir.swap(b - 1, b);
                        b -= 1;
                    }
                }
                let mut winding = 0i32;
                let mut span_start = 0i32;
                for k in 0..n_cross {
                    let prev = winding;
                    winding += self.cross_dir[k] as i32;
                    if prev == 0 && winding != 0 {
                        span_start = self.cross_x[k];
                    } else if prev != 0 && winding == 0 {
                        add_span(&mut self.acc[..span], span_start, self.cross_x[k]);
                    }
                }
            }

            let row_start = (row * target_w + col_lo) * 4;
            for (i, &sub_cov) in self.acc[..span].iter().enumerate() {
                let c = (sub_cov + (SUBSAMPLES / 2)) / SUBSAMPLES;
                let cov = if c > 255 { 255 } else if c < 0 { 0 } else { c as u8 };
                if cov > 0 {
                    let px_offset = row_start + i * 4;
                    if px_offset + 4 <= out_rgba.len() {
                        blend_pixel(&mut out_rgba[px_offset..px_offset + 4], color, cov);
                    }
                }
            }
        }
    }
}

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
    let start_px = (pa + 1).max(0);
    let end_px = pb.min(acc.len() as i32);
    for px in start_px..end_px {
        acc[px as usize] += FULL_COVER;
    }
    put(acc, pb, (xb & 0xFFFF) >> 8);
}

#[inline]
fn blend_pixel(dst: &mut [u8], src: (u8, u8, u8, u8), cov: u8) {
    let (sr, sg, sb, sa) = (src.0 as u32, src.1 as u32, src.2 as u32, src.3 as u32);
    let a_s = (sa * cov as u32) / 255;
    if a_s == 0 {
        return;
    }
    let dr = dst[0] as u32;
    let dg = dst[1] as u32;
    let db = dst[2] as u32;
    let da = dst[3] as u32;

    if da == 0 {
        dst[0] = sr as u8;
        dst[1] = sg as u8;
        dst[2] = sb as u8;
        dst[3] = a_s as u8;
        return;
    }

    if a_s == 255 {
        dst[0] = sr as u8;
        dst[1] = sg as u8;
        dst[2] = sb as u8;
        dst[3] = 255;
        return;
    }

    let inv_a_s = 255 - a_s;
    let a_out = a_s + (da * inv_a_s) / 255;
    if a_out == 0 {
        dst[0] = 0;
        dst[1] = 0;
        dst[2] = 0;
        dst[3] = 0;
        return;
    }

    let blend_c = |cs: u32, cd: u32| -> u8 {
        let num = cs * a_s + (cd * da * inv_a_s) / 255;
        let res = (num + a_out / 2) / a_out;
        if res > 255 { 255 } else { res as u8 }
    };

    dst[0] = blend_c(sr, dr);
    dst[1] = blend_c(sg, dg);
    dst[2] = blend_c(sb, db);
    dst[3] = if a_out > 255 { 255 } else { a_out as u8 };
}

pub fn generate_stroke_segments(
    path_segments: &SegmentBuffer,
    stroke_width: i32,
    stroke_buffer: &mut SegmentBuffer,
) -> Result<(), ()> {
    stroke_buffer.clear();
    let hw = (stroke_width as i64) / 2;
    if hw <= 0 {
        return Ok(());
    }

    for k in 0..path_segments.len {
        let x0 = path_segments.sx0[k];
        let y0 = path_segments.sy0[k];
        let x1 = path_segments.sx1[k];
        let y1 = path_segments.sy1[k];
        let dx = (x1 - x0) as i64;
        let dy = (y1 - y0) as i64;
        let len_sq = dx * dx + dy * dy;
        if len_sq == 0 {
            continue;
        }
        let len = isqrt64(len_sq as u64) as i64;
        if len == 0 {
            continue;
        }
        let nx = (-dy * hw / len) as i32;
        let ny = (dx * hw / len) as i32;

        let c0_x = x0 + nx;
        let c0_y = y0 + ny;
        let c1_x = x1 + nx;
        let c1_y = y1 + ny;
        let c2_x = x1 - nx;
        let c2_y = y1 - ny;
        let c3_x = x0 - nx;
        let c3_y = y0 - ny;

        stroke_buffer.push(c0_x, c0_y, c1_x, c1_y)?;
        stroke_buffer.push(c1_x, c1_y, c2_x, c2_y)?;
        stroke_buffer.push(c2_x, c2_y, c3_x, c3_y)?;
        stroke_buffer.push(c3_x, c3_y, c0_x, c0_y)?;

        // Bevel join with next segment if continuous
        if k + 1 < path_segments.len && path_segments.sx0[k + 1] == x1 && path_segments.sy0[k + 1] == y1 {
            let ndx = (path_segments.sx1[k + 1] - x1) as i64;
            let ndy = (path_segments.sy1[k + 1] - y1) as i64;
            let nlen_sq = ndx * ndx + ndy * ndy;
            if nlen_sq > 0 {
                let nlen = isqrt64(nlen_sq as u64) as i64;
                if nlen > 0 {
                    let nnx = (-ndy * hw / nlen) as i32;
                    let nny = (ndx * hw / nlen) as i32;
                    let _ = stroke_buffer.push(c1_x, c1_y, x1 + nnx, y1 + nny);
                    let _ = stroke_buffer.push(x1 + nnx, y1 + nny, x1, y1);
                    let _ = stroke_buffer.push(x1, y1, c1_x, c1_y);
                    let _ = stroke_buffer.push(c2_x, c2_y, x1 - nnx, y1 - nny);
                    let _ = stroke_buffer.push(x1 - nnx, y1 - nny, x1, y1);
                    let _ = stroke_buffer.push(x1, y1, c2_x, c2_y);
                }
            }
        }
    }
    Ok(())
}

pub fn emit_rect(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    rx: i32,
    ry: i32,
    transform: &impl Fn(i32, i32) -> (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    if w <= 0 || h <= 0 {
        return Ok(());
    }
    let rx = rx.min(w / 2);
    let ry = ry.min(h / 2);

    if rx <= 0 && ry <= 0 {
        let p0 = transform(x, y);
        let p1 = transform(x + w, y);
        let p2 = transform(x + w, y + h);
        let p3 = transform(x, y + h);
        segments.push(p0.0, p0.1, p1.0, p1.1)?;
        segments.push(p1.0, p1.1, p2.0, p2.1)?;
        segments.push(p2.0, p2.1, p3.0, p3.1)?;
        segments.push(p3.0, p3.1, p0.0, p0.1)?;
        return Ok(());
    }

    let kx = ((rx as i64 * KAPPA as i64) >> 16) as i32;
    let ky = ((ry as i64 * KAPPA as i64) >> 16) as i32;

    let pt0 = transform(x + rx, y);
    let pt1 = transform(x + w - rx, y);
    segments.push(pt0.0, pt0.1, pt1.0, pt1.1)?;

    let c0 = pt1;
    let c1 = transform(x + w - rx + kx, y);
    let c2 = transform(x + w, y + ry - ky);
    let c3 = transform(x + w, y + ry);
    flatten_cubic(c0, c1, c2, c3, segments)?;

    let pt2 = transform(x + w, y + h - ry);
    segments.push(c3.0, c3.1, pt2.0, pt2.1)?;

    let d0 = pt2;
    let d1 = transform(x + w, y + h - ry + ky);
    let d2 = transform(x + w - rx + kx, y + h);
    let d3 = transform(x + w - rx, y + h);
    flatten_cubic(d0, d1, d2, d3, segments)?;

    let pt3 = transform(x + rx, y + h);
    segments.push(d3.0, d3.1, pt3.0, pt3.1)?;

    let e0 = pt3;
    let e1 = transform(x + rx - kx, y + h);
    let e2 = transform(x, y + h - ry + ky);
    let e3 = transform(x, y + h - ry);
    flatten_cubic(e0, e1, e2, e3, segments)?;

    let pt4 = transform(x, y + ry);
    segments.push(e3.0, e3.1, pt4.0, pt4.1)?;

    let f0 = pt4;
    let f1 = transform(x, y + ry - ky);
    let f2 = transform(x + rx - kx, y);
    let f3 = pt0;
    flatten_cubic(f0, f1, f2, f3, segments)?;

    Ok(())
}

pub fn emit_rect_stroke(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    sw: i32,
    transform: &impl Fn(i32, i32) -> (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    let hw = sw / 2;
    let op0 = transform(x - hw, y - hw);
    let op1 = transform(x + w + hw, y - hw);
    let op2 = transform(x + w + hw, y + h + hw);
    let op3 = transform(x - hw, y + h + hw);
    segments.push(op0.0, op0.1, op1.0, op1.1)?;
    segments.push(op1.0, op1.1, op2.0, op2.1)?;
    segments.push(op2.0, op2.1, op3.0, op3.1)?;
    segments.push(op3.0, op3.1, op0.0, op0.1)?;

    if w > sw && h > sw {
        let ip0 = transform(x + hw, y + hw);
        let ip1 = transform(x + hw, y + h - hw);
        let ip2 = transform(x + w - hw, y + h - hw);
        let ip3 = transform(x + w - hw, y + hw);
        segments.push(ip0.0, ip0.1, ip1.0, ip1.1)?;
        segments.push(ip1.0, ip1.1, ip2.0, ip2.1)?;
        segments.push(ip2.0, ip2.1, ip3.0, ip3.1)?;
        segments.push(ip3.0, ip3.1, ip0.0, ip0.1)?;
    }
    Ok(())
}

pub fn emit_circle(
    cx: i32,
    cy: i32,
    r: i32,
    transform: &impl Fn(i32, i32) -> (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    if r <= 0 {
        return Ok(());
    }
    let kr = ((r as i64 * KAPPA as i64) >> 16) as i32;

    let p0 = transform(cx, cy - r);
    let p1 = transform(cx + kr, cy - r);
    let p2 = transform(cx + r, cy - kr);
    let p3 = transform(cx + r, cy);
    flatten_cubic(p0, p1, p2, p3, segments)?;

    let q0 = p3;
    let q1 = transform(cx + r, cy + kr);
    let q2 = transform(cx + kr, cy + r);
    let q3 = transform(cx, cy + r);
    flatten_cubic(q0, q1, q2, q3, segments)?;

    let r0 = q3;
    let r1 = transform(cx - kr, cy + r);
    let r2 = transform(cx - r, cy + kr);
    let r3 = transform(cx - r, cy);
    flatten_cubic(r0, r1, r2, r3, segments)?;

    let s0 = r3;
    let s1 = transform(cx - r, cy - kr);
    let s2 = transform(cx - kr, cy - r);
    let s3 = p0;
    flatten_cubic(s0, s1, s2, s3, segments)?;

    Ok(())
}

pub fn emit_circle_stroke(
    cx: i32,
    cy: i32,
    r: i32,
    sw: i32,
    transform: &impl Fn(i32, i32) -> (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    let hw = sw / 2;
    let r_out = r + hw;
    emit_circle(cx, cy, r_out, transform, segments)?;

    if r > hw {
        let r_in = r - hw;
        let kr = ((r_in as i64 * KAPPA as i64) >> 16) as i32;
        let p0 = transform(cx, cy - r_in);
        let p1 = transform(cx - kr, cy - r_in);
        let p2 = transform(cx - r_in, cy - kr);
        let p3 = transform(cx - r_in, cy);
        flatten_cubic(p0, p1, p2, p3, segments)?;

        let q0 = p3;
        let q1 = transform(cx - r_in, cy + kr);
        let q2 = transform(cx - kr, cy + r_in);
        let q3 = transform(cx, cy + r_in);
        flatten_cubic(q0, q1, q2, q3, segments)?;

        let r0 = q3;
        let r1 = transform(cx + kr, cy + r_in);
        let r2 = transform(cx + r_in, cy + kr);
        let r3 = transform(cx + r_in, cy);
        flatten_cubic(r0, r1, r2, r3, segments)?;

        let s0 = r3;
        let s1 = transform(cx + r_in, cy - kr);
        let s2 = transform(cx + kr, cy - r_in);
        let s3 = p0;
        flatten_cubic(s0, s1, s2, s3, segments)?;
    }
    Ok(())
}

pub fn emit_path_d(
    d_bytes: &[u8],
    transform: &impl Fn(i32, i32) -> (i32, i32),
    segments: &mut SegmentBuffer,
) -> Result<(), ()> {
    let mut scanner = NumberScanner::new(d_bytes);
    let mut cur_cmd = 0u8;
    let mut cur_orig = (0i32, 0i32);
    let mut cur_pt = (0i32, 0i32);
    let mut start_orig = (0i32, 0i32);
    let mut start_pt = (0i32, 0i32);
    let mut last_cubic_ctrl: Option<(i32, i32)> = None;
    let mut last_quad_ctrl: Option<(i32, i32)> = None;

    while scanner.pos < scanner.data.len() {
        scanner.skip_whitespace_and_commas();
        if scanner.pos >= scanner.data.len() {
            break;
        }

        let b = scanner.data[scanner.pos];
        if b.is_ascii_alphabetic() {
            cur_cmd = b;
            scanner.pos += 1;
        }

        match cur_cmd {
            b'M' => {
                let x = scanner.next_fixed().ok_or(())?;
                let y = scanner.next_fixed().ok_or(())?;
                let pt = transform(x, y);
                cur_orig = (x, y);
                cur_pt = pt;
                start_orig = cur_orig;
                start_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
                cur_cmd = b'L';
            }
            b'm' => {
                let dx = scanner.next_fixed().ok_or(())?;
                let dy = scanner.next_fixed().ok_or(())?;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1 + dy;
                let pt = transform(x, y);
                cur_orig = (x, y);
                cur_pt = pt;
                start_orig = cur_orig;
                start_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
                cur_cmd = b'l';
            }
            b'L' => {
                let x = scanner.next_fixed().ok_or(())?;
                let y = scanner.next_fixed().ok_or(())?;
                let pt = transform(x, y);
                segments.push(cur_pt.0, cur_pt.1, pt.0, pt.1)?;
                cur_orig = (x, y);
                cur_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            b'l' => {
                let dx = scanner.next_fixed().ok_or(())?;
                let dy = scanner.next_fixed().ok_or(())?;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1 + dy;
                let pt = transform(x, y);
                segments.push(cur_pt.0, cur_pt.1, pt.0, pt.1)?;
                cur_orig = (x, y);
                cur_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            b'H' => {
                let x = scanner.next_fixed().ok_or(())?;
                let y = cur_orig.1;
                let pt = transform(x, y);
                segments.push(cur_pt.0, cur_pt.1, pt.0, pt.1)?;
                cur_orig = (x, y);
                cur_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            b'h' => {
                let dx = scanner.next_fixed().ok_or(())?;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1;
                let pt = transform(x, y);
                segments.push(cur_pt.0, cur_pt.1, pt.0, pt.1)?;
                cur_orig = (x, y);
                cur_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            b'V' => {
                let y = scanner.next_fixed().ok_or(())?;
                let x = cur_orig.0;
                let pt = transform(x, y);
                segments.push(cur_pt.0, cur_pt.1, pt.0, pt.1)?;
                cur_orig = (x, y);
                cur_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            b'v' => {
                let dy = scanner.next_fixed().ok_or(())?;
                let x = cur_orig.0;
                let y = cur_orig.1 + dy;
                let pt = transform(x, y);
                segments.push(cur_pt.0, cur_pt.1, pt.0, pt.1)?;
                cur_orig = (x, y);
                cur_pt = pt;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            b'C' => {
                let x1 = scanner.next_fixed().ok_or(())?;
                let y1 = scanner.next_fixed().ok_or(())?;
                let x2 = scanner.next_fixed().ok_or(())?;
                let y2 = scanner.next_fixed().ok_or(())?;
                let x = scanner.next_fixed().ok_or(())?;
                let y = scanner.next_fixed().ok_or(())?;
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x2, y2);
                let p3 = transform(x, y);
                flatten_cubic(p0, p1, p2, p3, segments)?;
                cur_orig = (x, y);
                cur_pt = p3;
                last_cubic_ctrl = Some((x2, y2));
                last_quad_ctrl = None;
            }
            b'c' => {
                let dx1 = scanner.next_fixed().ok_or(())?;
                let dy1 = scanner.next_fixed().ok_or(())?;
                let dx2 = scanner.next_fixed().ok_or(())?;
                let dy2 = scanner.next_fixed().ok_or(())?;
                let dx = scanner.next_fixed().ok_or(())?;
                let dy = scanner.next_fixed().ok_or(())?;
                let x1 = cur_orig.0 + dx1;
                let y1 = cur_orig.1 + dy1;
                let x2 = cur_orig.0 + dx2;
                let y2 = cur_orig.1 + dy2;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1 + dy;
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x2, y2);
                let p3 = transform(x, y);
                flatten_cubic(p0, p1, p2, p3, segments)?;
                cur_orig = (x, y);
                cur_pt = p3;
                last_cubic_ctrl = Some((x2, y2));
                last_quad_ctrl = None;
            }
            b'S' => {
                let x2 = scanner.next_fixed().ok_or(())?;
                let y2 = scanner.next_fixed().ok_or(())?;
                let x = scanner.next_fixed().ok_or(())?;
                let y = scanner.next_fixed().ok_or(())?;
                let (x1, y1) = match last_cubic_ctrl {
                    Some((lx, ly)) => (2 * cur_orig.0 - lx, 2 * cur_orig.1 - ly),
                    None => cur_orig,
                };
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x2, y2);
                let p3 = transform(x, y);
                flatten_cubic(p0, p1, p2, p3, segments)?;
                cur_orig = (x, y);
                cur_pt = p3;
                last_cubic_ctrl = Some((x2, y2));
                last_quad_ctrl = None;
            }
            b's' => {
                let dx2 = scanner.next_fixed().ok_or(())?;
                let dy2 = scanner.next_fixed().ok_or(())?;
                let dx = scanner.next_fixed().ok_or(())?;
                let dy = scanner.next_fixed().ok_or(())?;
                let x2 = cur_orig.0 + dx2;
                let y2 = cur_orig.1 + dy2;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1 + dy;
                let (x1, y1) = match last_cubic_ctrl {
                    Some((lx, ly)) => (2 * cur_orig.0 - lx, 2 * cur_orig.1 - ly),
                    None => cur_orig,
                };
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x2, y2);
                let p3 = transform(x, y);
                flatten_cubic(p0, p1, p2, p3, segments)?;
                cur_orig = (x, y);
                cur_pt = p3;
                last_cubic_ctrl = Some((x2, y2));
                last_quad_ctrl = None;
            }
            b'Q' => {
                let x1 = scanner.next_fixed().ok_or(())?;
                let y1 = scanner.next_fixed().ok_or(())?;
                let x = scanner.next_fixed().ok_or(())?;
                let y = scanner.next_fixed().ok_or(())?;
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x, y);
                flatten_quad(p0, p1, p2, segments)?;
                cur_orig = (x, y);
                cur_pt = p2;
                last_quad_ctrl = Some((x1, y1));
                last_cubic_ctrl = None;
            }
            b'q' => {
                let dx1 = scanner.next_fixed().ok_or(())?;
                let dy1 = scanner.next_fixed().ok_or(())?;
                let dx = scanner.next_fixed().ok_or(())?;
                let dy = scanner.next_fixed().ok_or(())?;
                let x1 = cur_orig.0 + dx1;
                let y1 = cur_orig.1 + dy1;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1 + dy;
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x, y);
                flatten_quad(p0, p1, p2, segments)?;
                cur_orig = (x, y);
                cur_pt = p2;
                last_quad_ctrl = Some((x1, y1));
                last_cubic_ctrl = None;
            }
            b'T' => {
                let x = scanner.next_fixed().ok_or(())?;
                let y = scanner.next_fixed().ok_or(())?;
                let (x1, y1) = match last_quad_ctrl {
                    Some((lx, ly)) => (2 * cur_orig.0 - lx, 2 * cur_orig.1 - ly),
                    None => cur_orig,
                };
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x, y);
                flatten_quad(p0, p1, p2, segments)?;
                cur_orig = (x, y);
                cur_pt = p2;
                last_quad_ctrl = Some((x1, y1));
                last_cubic_ctrl = None;
            }
            b't' => {
                let dx = scanner.next_fixed().ok_or(())?;
                let dy = scanner.next_fixed().ok_or(())?;
                let x = cur_orig.0 + dx;
                let y = cur_orig.1 + dy;
                let (x1, y1) = match last_quad_ctrl {
                    Some((lx, ly)) => (2 * cur_orig.0 - lx, 2 * cur_orig.1 - ly),
                    None => cur_orig,
                };
                let p0 = cur_pt;
                let p1 = transform(x1, y1);
                let p2 = transform(x, y);
                flatten_quad(p0, p1, p2, segments)?;
                cur_orig = (x, y);
                cur_pt = p2;
                last_quad_ctrl = Some((x1, y1));
                last_cubic_ctrl = None;
            }
            b'Z' | b'z' => {
                if cur_pt != start_pt {
                    segments.push(cur_pt.0, cur_pt.1, start_pt.0, start_pt.1)?;
                    cur_pt = start_pt;
                }
                cur_orig = start_orig;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            _ => return Err(()),
        }
    }
    Ok(())
}

/// Reads dimensions from the `<svg>` header.
/// Returns `(width, height)`.
pub fn decode_svg_header(data: &[u8]) -> Result<(u32, u32), ()> {
    let mut pos = 0;
    while pos < data.len() {
        if data[pos] == b'<' {
            if data[pos..].starts_with(b"<!--") {
                if let Some(end) = find_subslice(&data[pos..], b"-->") {
                    pos += end + 4;
                    continue;
                } else {
                    return Err(());
                }
            }
            if data[pos..].starts_with(b"<?") {
                if let Some(end) = find_subslice(&data[pos..], b"?>") {
                    pos += end + 2;
                    continue;
                } else {
                    return Err(());
                }
            }
            if data[pos..].starts_with(b"<!") {
                if let Some(end) = data[pos..].iter().position(|&b| b == b'>') {
                    pos += end + 1;
                    continue;
                } else {
                    return Err(());
                }
            }

            let tag_start = pos + 1;
            if let Some(end_rel) = data[tag_start..].iter().position(|&b| b == b'>') {
                let tag = &data[tag_start..tag_start + end_rel];
                let tag_name = if tag.starts_with(b"svg:") { &tag[4..] } else { tag };
                if tag_name.starts_with(b"svg") && (tag_name.len() == 3 || tag_name[3].is_ascii_whitespace() || tag_name[3] == b'/') {
                    let w_attr = get_attr(tag, b"width").and_then(parse_dimension);
                    let h_attr = get_attr(tag, b"height").and_then(parse_dimension);
                    let vb_attr = get_attr(tag, b"viewBox").and_then(parse_view_box);

                    let (w, h) = match (w_attr, h_attr, vb_attr) {
                        (Some(w), Some(h), _) => (w, h),
                        (Some(w), None, Some((_, _, vb_w, vb_h))) => {
                            let h = ((w as u64 * (vb_h >> 16) as u64) / (vb_w >> 16) as u64) as u32;
                            (w, h.max(1))
                        }
                        (None, Some(h), Some((_, _, vb_w, vb_h))) => {
                            let w = ((h as u64 * (vb_w >> 16) as u64) / (vb_h >> 16) as u64) as u32;
                            (w.max(1), h)
                        }
                        (None, None, Some((_, _, vb_w, vb_h))) => {
                            (((vb_w + 32768) >> 16) as u32, ((vb_h + 32768) >> 16) as u32)
                        }
                        _ => return Err(()),
                    };
                    if w > 0 && h > 0 && w <= MAX_WIDTH as u32 && h <= 4096 {
                        return Ok((w, h));
                    } else {
                        return Err(());
                    }
                }
                pos = tag_start + end_rel + 1;
            } else {
                return Err(());
            }
        } else {
            pos += 1;
        }
    }
    Err(())
}

fn find_svg_view_box(data: &[u8]) -> Option<(i32, i32, i32, i32)> {
    let mut pos = 0;
    while pos < data.len() {
        if data[pos] == b'<' {
            if data[pos..].starts_with(b"<!--") {
                if let Some(end) = find_subslice(&data[pos..], b"-->") {
                    pos += end + 4;
                    continue;
                } else {
                    return None;
                }
            }
            if data[pos..].starts_with(b"<?") || data[pos..].starts_with(b"<!") {
                if let Some(end) = data[pos..].iter().position(|&b| b == b'>') {
                    pos += end + 1;
                    continue;
                } else {
                    return None;
                }
            }

            let tag_start = pos + 1;
            if let Some(end_rel) = data[tag_start..].iter().position(|&b| b == b'>') {
                let tag = &data[tag_start..tag_start + end_rel];
                let tag_name = if tag.starts_with(b"svg:") { &tag[4..] } else { tag };
                if tag_name.starts_with(b"svg") && (tag_name.len() == 3 || tag_name[3].is_ascii_whitespace() || tag_name[3] == b'/') {
                    return get_attr(tag, b"viewBox").and_then(parse_view_box);
                }
                pos = tag_start + end_rel + 1;
            } else {
                return None;
            }
        } else {
            pos += 1;
        }
    }
    None
}

#[derive(Clone, Copy)]
struct StyleState {
    fill: Option<Color>,
    stroke: Option<Color>,
    stroke_width: Option<i32>,
}

/// Decodes an SVG image into the supplied RGBA buffer.
/// Returns `(width, height, bytes_written)`.
pub fn decode_svg(data: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    let (target_w, target_h) = decode_svg_header(data)?;
    let pixel_count = (target_w as usize).checked_mul(target_h as usize).ok_or(())?;
    let required = pixel_count.checked_mul(4).ok_or(())?;
    if required > out_rgba.len() {
        return Err(());
    }

    for px in out_rgba[..required].iter_mut() {
        *px = 0;
    }

    let (vb_min_x, vb_min_y, vb_w, vb_h) = find_svg_view_box(data).unwrap_or((
        0,
        0,
        (target_w as i32) << 16,
        (target_h as i32) << 16,
    ));
    if vb_w <= 0 || vb_h <= 0 {
        return Err(());
    }

    let scale_x = (((target_w as i64) << 16) << 16) / (vb_w as i64);
    let scale_y = (((target_h as i64) << 16) << 16) / (vb_h as i64);
    let scale_stroke = (scale_x + scale_y) / 2;

    let transform = |x: i32, y: i32| -> (i32, i32) {
        let xr = (x - vb_min_x) as i64;
        let yr = (y - vb_min_y) as i64;
        let sx = ((xr * scale_x) >> 16) as i32;
        let sy = ((yr * scale_y) >> 16) as i32;
        (sx, sy)
    };

    let mut fill_buf = SegmentBuffer::new();
    let mut stroke_buf = SegmentBuffer::new();
    let mut rasterizer = SvgRasterizer::new();

    let mut style_stack = [StyleState {
        fill: None,
        stroke: None,
        stroke_width: None,
    }; 8];
    let mut style_depth = 0usize;

    let mut pos = 0;
    while pos < data.len() {
        if data[pos] == b'<' {
            if data[pos..].starts_with(b"<!--") {
                if let Some(end) = find_subslice(&data[pos..], b"-->") {
                    pos += end + 4;
                    continue;
                } else {
                    return Err(());
                }
            }
            if data[pos..].starts_with(b"<?") || data[pos..].starts_with(b"<!") {
                if let Some(end) = data[pos..].iter().position(|&b| b == b'>') {
                    pos += end + 1;
                    continue;
                } else {
                    return Err(());
                }
            }

            let tag_start = pos + 1;
            if let Some(end_rel) = data[tag_start..].iter().position(|&b| b == b'>') {
                let tag = &data[tag_start..tag_start + end_rel];
                pos = tag_start + end_rel + 1;

                let tag_trimmed = if tag.starts_with(b"svg:") { &tag[4..] } else { tag };
                let mut elem_end = 0;
                while elem_end < tag_trimmed.len() && !tag_trimmed[elem_end].is_ascii_whitespace() && tag_trimmed[elem_end] != b'/' && tag_trimmed[elem_end] != b'>' {
                    elem_end += 1;
                }
                let elem = &tag_trimmed[..elem_end];

                if elem == b"g" {
                    if style_depth + 1 < style_stack.len() {
                        let cur = style_stack[style_depth];
                        let new_fill = get_attr_or_style(tag, b"fill").and_then(parse_color).or(cur.fill);
                        let new_stroke = get_attr_or_style(tag, b"stroke").and_then(parse_color).or(cur.stroke);
                        let new_sw = get_attr_or_style(tag, b"stroke-width").and_then(parse_fixed).or(cur.stroke_width);
                        style_depth += 1;
                        style_stack[style_depth] = StyleState {
                            fill: new_fill,
                            stroke: new_stroke,
                            stroke_width: new_sw,
                        };
                    }
                } else if elem == b"/g" {
                    if style_depth > 0 {
                        style_depth -= 1;
                    }
                } else if elem == b"rect" {
                    let cur = style_stack[style_depth];
                    let fill = get_attr_or_style(tag, b"fill").and_then(parse_color).or(cur.fill).unwrap_or(Color::Rgba(0, 0, 0, 255));
                    let stroke = get_attr_or_style(tag, b"stroke").and_then(parse_color).or(cur.stroke).unwrap_or(Color::None);
                    let stroke_width = get_attr_or_style(tag, b"stroke-width").and_then(parse_fixed).or(cur.stroke_width).unwrap_or(ONE);

                    let x = get_attr(tag, b"x").and_then(parse_fixed).unwrap_or(0);
                    let y = get_attr(tag, b"y").and_then(parse_fixed).unwrap_or(0);
                    let w = get_attr(tag, b"width").and_then(parse_fixed).unwrap_or(0);
                    let h = get_attr(tag, b"height").and_then(parse_fixed).unwrap_or(0);
                    let rx = get_attr(tag, b"rx").and_then(parse_fixed).unwrap_or(0);
                    let ry = get_attr(tag, b"ry").and_then(parse_fixed).unwrap_or(rx);
                    let rx = if rx == 0 && ry > 0 { ry } else { rx };

                    if w > 0 && h > 0 {
                        if rx == 0 && ry == 0 {
                            if let Color::Rgba(r, g, b, a) = fill {
                                fill_buf.clear();
                                emit_rect(x, y, w, h, 0, 0, &transform, &mut fill_buf)?;
                                rasterizer.rasterize(&fill_buf, target_w as usize, target_h as usize, (r, g, b, a), out_rgba);
                            }
                            if let Color::Rgba(r, g, b, a) = stroke {
                                if stroke_width > 0 {
                                    stroke_buf.clear();
                                    emit_rect_stroke(x, y, w, h, stroke_width, &transform, &mut stroke_buf)?;
                                    rasterizer.rasterize(&stroke_buf, target_w as usize, target_h as usize, (r, g, b, a), out_rgba);
                                }
                            }
                        } else {
                            fill_buf.clear();
                            emit_rect(x, y, w, h, rx, ry, &transform, &mut fill_buf)?;
                            if let Color::Rgba(r, g, b, a) = fill {
                                rasterizer.rasterize(&fill_buf, target_w as usize, target_h as usize, (r, g, b, a), out_rgba);
                            }
                            if let Color::Rgba(r, g, b, a) = stroke {
                                if stroke_width > 0 {
                                    let sw_screen = ((stroke_width as i64 * scale_stroke) >> 16) as i32;
                                    generate_stroke_segments(&fill_buf, sw_screen, &mut stroke_buf)?;
                                    rasterizer.rasterize(&stroke_buf, target_w as usize, target_h as usize, (r, g, b, a), out_rgba);
                                }
                            }
                        }
                    }
                } else if elem == b"circle" {
                    let cur = style_stack[style_depth];
                    let fill = get_attr_or_style(tag, b"fill").and_then(parse_color).or(cur.fill).unwrap_or(Color::Rgba(0, 0, 0, 255));
                    let stroke = get_attr_or_style(tag, b"stroke").and_then(parse_color).or(cur.stroke).unwrap_or(Color::None);
                    let stroke_width = get_attr_or_style(tag, b"stroke-width").and_then(parse_fixed).or(cur.stroke_width).unwrap_or(ONE);

                    let cx = get_attr(tag, b"cx").and_then(parse_fixed).unwrap_or(0);
                    let cy = get_attr(tag, b"cy").and_then(parse_fixed).unwrap_or(0);
                    let r = get_attr(tag, b"r").and_then(parse_fixed).unwrap_or(0);

                    if r > 0 {
                        if let Color::Rgba(cr, cg, cb, ca) = fill {
                            fill_buf.clear();
                            emit_circle(cx, cy, r, &transform, &mut fill_buf)?;
                            rasterizer.rasterize(&fill_buf, target_w as usize, target_h as usize, (cr, cg, cb, ca), out_rgba);
                        }
                        if let Color::Rgba(cr, cg, cb, ca) = stroke {
                            if stroke_width > 0 {
                                stroke_buf.clear();
                                emit_circle_stroke(cx, cy, r, stroke_width, &transform, &mut stroke_buf)?;
                                rasterizer.rasterize(&stroke_buf, target_w as usize, target_h as usize, (cr, cg, cb, ca), out_rgba);
                            }
                        }
                    }
                } else if elem == b"path" {
                    let cur = style_stack[style_depth];
                    let fill = get_attr_or_style(tag, b"fill").and_then(parse_color).or(cur.fill).unwrap_or(Color::Rgba(0, 0, 0, 255));
                    let stroke = get_attr_or_style(tag, b"stroke").and_then(parse_color).or(cur.stroke).unwrap_or(Color::None);
                    let stroke_width = get_attr_or_style(tag, b"stroke-width").and_then(parse_fixed).or(cur.stroke_width).unwrap_or(ONE);

                    if let Some(d_bytes) = get_attr(tag, b"d") {
                        fill_buf.clear();
                        emit_path_d(d_bytes, &transform, &mut fill_buf)?;

                        if let Color::Rgba(r, g, b, a) = fill {
                            rasterizer.rasterize(&fill_buf, target_w as usize, target_h as usize, (r, g, b, a), out_rgba);
                        }
                        if let Color::Rgba(r, g, b, a) = stroke {
                            if stroke_width > 0 {
                                let sw_screen = ((stroke_width as i64 * scale_stroke) >> 16) as i32;
                                generate_stroke_segments(&fill_buf, sw_screen, &mut stroke_buf)?;
                                rasterizer.rasterize(&stroke_buf, target_w as usize, target_h as usize, (r, g, b, a), out_rgba);
                            }
                        }
                    }
                }
            } else {
                return Err(());
            }
        } else {
            pos += 1;
        }
    }

    Ok((target_w, target_h, required))
}
