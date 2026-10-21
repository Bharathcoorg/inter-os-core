//! The platform's image decoding.
//!
//! WHY this lives in the platform and not in the gallery: decoding an image is a
//! service the whole device needs. The gallery reads photographs, the launcher
//! paints a document the user chose as a wallpaper, and the file manager shows what
//! a document is. Three programs that each carried their own decoder would be three
//! programs that could each be wrong about what a QOI file is.
//!
//! Every decoder here is a real decoder: it reads the container's own fields and
//! refuses a file it cannot make sense of rather than inventing a picture.

/// The image file extensions the platform recognises.
pub const IMAGE_SUFFIXES: [&[u8]; 9] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp", b".qoi", b".ppm", b".raw",
];


/// Whether an entry name carries a recognised image extension (case-insensitive).
pub fn has_image_suffix(name: &[u8]) -> bool {
    IMAGE_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Whether an entry name carries a recognised video extension (case-insensitive).

/// Appends `bytes` to `buf` at `n` and returns the new length, truncating
/// rather than overrunning.
pub fn append(buf: &mut [u8], n: usize, bytes: &[u8]) -> usize {
    let take = bytes.len().min(buf.len().saturating_sub(n));
    buf[n..n + take].copy_from_slice(&bytes[..take]);
    n + take
}

/// Appends the decimal form of `value` to `buf` at `n`.
pub fn append_u64(buf: &mut [u8], mut n: usize, mut value: u64) -> usize {
    if value == 0 {
        return append(buf, n, b"0");
    }
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    while value > 0 && len < 20 {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

/// A decoded image: its size and its pixels, four bytes each, red first.
///
/// WHY a reference and not an owned buffer: the apps in this system have no heap,
/// so a decoded image lives in a buffer the caller owns and this describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bitmap<'a> {
    /// The width in pixels.
    pub w: u32,
    /// The height in pixels.
    pub h: u32,
    /// The pixels, four bytes each, in row-major order.
    pub rgba: &'a [u8],
}

impl Bitmap<'_> {
    /// The pixel at (x, y), scaled from the image's own size to the surface.
    ///
    /// WHY the image is scaled rather than tiled: a wallpaper is one picture across
    /// the surface, and a picture that repeats is a pattern, not a wallpaper.
    pub fn pixel(&self, x: usize, y: usize, surface_w: usize, surface_h: usize) -> u32 {
        if self.w == 0 || self.h == 0 || self.rgba.is_empty() {
            return 0xFF000000;
        }
        let sx = ((x as u64 * (self.w as u64 - 1)) / (surface_w.max(1) as u64 - 1).max(1)) as usize;
        let sy = ((y as u64 * (self.h as u64 - 1)) / (surface_h.max(1) as u64 - 1).max(1)) as usize;
        let offset = (sy * self.w as usize + sx) * 4;
        if offset + 4 > self.rgba.len() {
            return 0xFF000000;
        }
        let r = self.rgba[offset] as u32;
        let g = self.rgba[offset + 1] as u32;
        let b = self.rgba[offset + 2] as u32;
        let a = self.rgba[offset + 3] as u32;
        (a << 24) | (r << 16) | (g << 8) | b
    }
}

pub fn qoi_color_hash(r: u8, g: u8, b: u8, a: u8) -> usize {
    (r as usize * 3 + g as usize * 5 + b as usize * 7 + a as usize * 11) % 64
}

/// Reads and validates the 14-byte QOI header.
/// Returns (width, height, channels, colorspace).
pub fn decode_qoi_header(data: &[u8]) -> Result<(u32, u32, u8, u8), ()> {
    if data.len() < 14 || &data[0..4] != b"qoif" {
        return Err(());
    }
    let width = u32::from_be_bytes(data[4..8].try_into().map_err(|_| ())?);
    let height = u32::from_be_bytes(data[8..12].try_into().map_err(|_| ())?);
    let channels = data[12];
    let colorspace = data[13];
    if width == 0 || height == 0 || (channels != 3 && channels != 4) {
        return Err(());
    }
    Ok((width, height, channels, colorspace))
}

/// Decodes a Quite OK Image (QOI) stream into an RGBA (4 bytes per pixel) destination buffer.
/// Returns the number of RGBA bytes written to `out_rgba`.
pub fn decode_qoi(data: &[u8], out_rgba: &mut [u8]) -> Result<usize, ()> {
    let (width, height, _, _) = decode_qoi_header(data)?;
    let pixel_count = (width as usize).checked_mul(height as usize).ok_or(())?;
    let required_bytes = pixel_count.checked_mul(4).ok_or(())?;
    if out_rgba.len() < required_bytes {
        return Err(());
    }

    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0u8, 0u8, 255u8];
    let mut in_pos = 14usize;
    let mut out_pos = 0usize;
    let end = data.len().saturating_sub(8);

    while in_pos < end && out_pos < required_bytes {
        let b1 = data[in_pos];
        in_pos += 1;

        if b1 == 0xFF {
            // QOI_OP_RGBA
            if in_pos + 4 > end {
                return Err(());
            }
            prev = [data[in_pos], data[in_pos + 1], data[in_pos + 2], data[in_pos + 3]];
            in_pos += 4;
            index[qoi_color_hash(prev[0], prev[1], prev[2], prev[3])] = prev;
            out_rgba[out_pos..out_pos + 4].copy_from_slice(&prev);
            out_pos += 4;
        } else if b1 == 0xFE {
            // QOI_OP_RGB
            if in_pos + 3 > end {
                return Err(());
            }
            prev = [data[in_pos], data[in_pos + 1], data[in_pos + 2], prev[3]];
            in_pos += 3;
            index[qoi_color_hash(prev[0], prev[1], prev[2], prev[3])] = prev;
            out_rgba[out_pos..out_pos + 4].copy_from_slice(&prev);
            out_pos += 4;
        } else {
            match b1 & 0b1100_0000 {
                0b0000_0000 => {
                    // QOI_OP_INDEX
                    let idx = (b1 & 0x3F) as usize;
                    prev = index[idx];
                    out_rgba[out_pos..out_pos + 4].copy_from_slice(&prev);
                    out_pos += 4;
                }
                0b0100_0000 => {
                    // QOI_OP_DIFF
                    let dr = (((b1 >> 4) & 0x03) as i8) - 2;
                    let dg = (((b1 >> 2) & 0x03) as i8) - 2;
                    let db = ((b1 & 0x03) as i8) - 2;
                    prev[0] = prev[0].wrapping_add(dr as u8);
                    prev[1] = prev[1].wrapping_add(dg as u8);
                    prev[2] = prev[2].wrapping_add(db as u8);
                    index[qoi_color_hash(prev[0], prev[1], prev[2], prev[3])] = prev;
                    out_rgba[out_pos..out_pos + 4].copy_from_slice(&prev);
                    out_pos += 4;
                }
                0b1000_0000 => {
                    // QOI_OP_LUMA
                    if in_pos >= end {
                        return Err(());
                    }
                    let b2 = data[in_pos];
                    in_pos += 1;
                    let dg = ((b1 & 0x3F) as i8) - 32;
                    let dr = dg + (((b2 >> 4) & 0x0F) as i8) - 8;
                    let db = dg + ((b2 & 0x0F) as i8) - 8;
                    prev[0] = prev[0].wrapping_add(dr as u8);
                    prev[1] = prev[1].wrapping_add(dg as u8);
                    prev[2] = prev[2].wrapping_add(db as u8);
                    index[qoi_color_hash(prev[0], prev[1], prev[2], prev[3])] = prev;
                    out_rgba[out_pos..out_pos + 4].copy_from_slice(&prev);
                    out_pos += 4;
                }
                0b1100_0000 => {
                    // QOI_OP_RUN
                    let count = ((b1 & 0x3F) as usize) + 1;
                    for _ in 0..count {
                        if out_pos + 4 > out_rgba.len() {
                            return Err(());
                        }
                        out_rgba[out_pos..out_pos + 4].copy_from_slice(&prev);
                        out_pos += 4;
                    }
                }
                _ => return Err(()),
            }
        }
    }

    Ok(out_pos)
}

fn skip_ppm_whitespace_and_comments(data: &[u8], mut pos: usize) -> usize {
    while pos < data.len() {
        let b = data[pos];
        if b.is_ascii_whitespace() {
            pos += 1;
        } else if b == b'#' {
            pos += 1;
            while pos < data.len() && data[pos] != b'\n' {
                pos += 1;
            }
            if pos < data.len() && data[pos] == b'\n' {
                pos += 1;
            }
        } else {
            break;
        }
    }
    pos
}

fn parse_ppm_u32(data: &[u8], pos: &mut usize) -> Result<u32, ()> {
    *pos = skip_ppm_whitespace_and_comments(data, *pos);
    if *pos >= data.len() || !data[*pos].is_ascii_digit() {
        return Err(());
    }
    let mut val: u32 = 0;
    while *pos < data.len() && data[*pos].is_ascii_digit() {
        val = val.checked_mul(10).ok_or(())?;
        val = val.checked_add((data[*pos] - b'0') as u32).ok_or(())?;
        *pos += 1;
    }
    Ok(val)
}

/// Reads and validates the PPM (P6 binary) header.
/// Returns (width, height, data_offset).
pub fn decode_ppm_header(data: &[u8]) -> Result<(u32, u32, usize), ()> {
    if data.len() < 3 || data[0] != b'P' || data[1] != b'6' {
        return Err(());
    }
    if !data[2].is_ascii_whitespace() && data[2] != b'#' {
        return Err(());
    }
    let mut pos = 2;
    let width = parse_ppm_u32(data, &mut pos)?;
    let height = parse_ppm_u32(data, &mut pos)?;
    let maxval = parse_ppm_u32(data, &mut pos)?;
    if width == 0 || height == 0 || maxval == 0 || maxval > 255 {
        return Err(());
    }
    if pos >= data.len() || !data[pos].is_ascii_whitespace() {
        return Err(());
    }
    pos += 1; // binary pixel data starts after this single whitespace byte
    Ok((width, height, pos))
}

/// Decodes a PPM (P6 binary) stream into an RGBA (4 bytes per pixel) destination buffer.
/// Returns the number of RGBA bytes written to `out_rgba`.
pub fn decode_ppm(data: &[u8], out_rgba: &mut [u8]) -> Result<usize, ()> {
    let (width, height, offset) = decode_ppm_header(data)?;
    let pixel_count = (width as usize).checked_mul(height as usize).ok_or(())?;
    let required_rgb = pixel_count.checked_mul(3).ok_or(())?;
    let required_rgba = pixel_count.checked_mul(4).ok_or(())?;
    if data.len() < offset + required_rgb || out_rgba.len() < required_rgba {
        return Err(());
    }
    let rgb = &data[offset..offset + required_rgb];
    for i in 0..pixel_count {
        let r = rgb[i * 3];
        let g = rgb[i * 3 + 1];
        let b = rgb[i * 3 + 2];
        out_rgba[i * 4] = r;
        out_rgba[i * 4 + 1] = g;
        out_rgba[i * 4 + 2] = b;
        out_rgba[i * 4 + 3] = 255;
    }
    Ok(required_rgba)
}

/// Encodes an RGB or RGBA pixel buffer into a PPM (P6 binary) format.
/// Returns the number of bytes written to `out`.
pub fn encode_ppm(
    width: u32,
    height: u32,
    rgb_or_rgba: &[u8],
    is_rgba: bool,
    out: &mut [u8],
) -> Result<usize, ()> {
    let pixel_count = (width as usize).checked_mul(height as usize).ok_or(())?;
    let in_stride = if is_rgba { 4 } else { 3 };
    if rgb_or_rgba.len() < pixel_count.checked_mul(in_stride).ok_or(())? {
        return Err(());
    }
    let mut header = [0u8; 32];
    let mut hlen = append(&mut header, 0, b"P6\n");
    hlen = append_u64(&mut header, hlen, width as u64);
    hlen = append(&mut header, hlen, b" ");
    hlen = append_u64(&mut header, hlen, height as u64);
    hlen = append(&mut header, hlen, b"\n255\n");
    let total_len = hlen
        .checked_add(pixel_count.checked_mul(3).ok_or(())?)
        .ok_or(())?;
    if out.len() < total_len {
        return Err(());
    }
    out[..hlen].copy_from_slice(&header[..hlen]);
    let mut out_pos = hlen;
    for i in 0..pixel_count {
        out[out_pos] = rgb_or_rgba[i * in_stride];
        out[out_pos + 1] = rgb_or_rgba[i * in_stride + 1];
        out[out_pos + 2] = rgb_or_rgba[i * in_stride + 2];
        out_pos += 3;
    }
    Ok(out_pos)
}

/// Reads and validates the Windows BMP header (24bpp or 32bpp uncompressed).
/// Returns (width, height, bpp, data_offset).
pub fn decode_bmp_header(data: &[u8]) -> Result<(u32, u32, u16, usize), ()> {
    if data.len() < 54 || data[0] != b'B' || data[1] != b'M' {
        return Err(());
    }
    let data_offset = u32::from_le_bytes(data[10..14].try_into().map_err(|_| ())?) as usize;
    let header_size = u32::from_le_bytes(data[14..18].try_into().map_err(|_| ())?);
    if header_size < 40 || data_offset < 54 || data_offset > data.len() {
        return Err(());
    }
    let raw_w = i32::from_le_bytes(data[18..22].try_into().map_err(|_| ())?);
    let raw_h = i32::from_le_bytes(data[22..26].try_into().map_err(|_| ())?);
    if raw_w <= 0 || raw_h == 0 {
        return Err(());
    }
    let width = raw_w as u32;
    let height = raw_h.unsigned_abs();
    let planes = u16::from_le_bytes(data[26..28].try_into().map_err(|_| ())?);
    let bpp = u16::from_le_bytes(data[28..30].try_into().map_err(|_| ())?);
    let compression = u32::from_le_bytes(data[30..34].try_into().map_err(|_| ())?);
    if planes != 1 || (bpp != 24 && bpp != 32) || compression != 0 {
        return Err(());
    }
    Ok((width, height, bpp, data_offset))
}

/// Decodes a BMP stream into an RGBA (4 bytes per pixel) destination buffer.
/// Returns the number of RGBA bytes written to `out_rgba`.
pub fn decode_bmp(data: &[u8], out_rgba: &mut [u8]) -> Result<usize, ()> {
    let (width, height, bpp, offset) = decode_bmp_header(data)?;
    let raw_h = i32::from_le_bytes(data[22..26].try_into().map_err(|_| ())?);
    let bottom_up = raw_h > 0;
    let bytes_per_pixel = (bpp / 8) as usize;
    let row_stride = ((width as usize * bytes_per_pixel + 3) / 4) * 4;
    let total_pixel_bytes = (height as usize).checked_mul(row_stride).ok_or(())?;
    if data.len() < offset + total_pixel_bytes {
        return Err(());
    }
    let required_rgba = (width as usize)
        .checked_mul(height as usize)
        .ok_or(())?
        .checked_mul(4)
        .ok_or(())?;
    if out_rgba.len() < required_rgba {
        return Err(());
    }
    for row in 0..height as usize {
        let src_row = if bottom_up {
            height as usize - 1 - row
        } else {
            row
        };
        let src_row_start = offset + src_row * row_stride;
        let dst_row_start = row * (width as usize) * 4;
        for col in 0..width as usize {
            let src_idx = src_row_start + col * bytes_per_pixel;
            let dst_idx = dst_row_start + col * 4;
            let b = data[src_idx];
            let g = data[src_idx + 1];
            let r = data[src_idx + 2];
            let a = if bpp == 32 { data[src_idx + 3] } else { 255 };
            out_rgba[dst_idx] = r;
            out_rgba[dst_idx + 1] = g;
            out_rgba[dst_idx + 2] = b;
            out_rgba[dst_idx + 3] = a;
        }
    }
    Ok(required_rgba)
}

/// Decodes raw pixel data into an RGBA (4 bytes per pixel) destination buffer.
pub fn decode_raw(
    data: &[u8],
    width: u32,
    height: u32,
    bpp: u8,
    out_rgba: &mut [u8],
) -> Result<usize, ()> {
    let pixel_count = (width as usize).checked_mul(height as usize).ok_or(())?;
    let required_rgba = pixel_count.checked_mul(4).ok_or(())?;
    if out_rgba.len() < required_rgba {
        return Err(());
    }
    if bpp == 24 {
        if data.len() < pixel_count.checked_mul(3).ok_or(())? {
            return Err(());
        }
        for i in 0..pixel_count {
            out_rgba[i * 4] = data[i * 3];
            out_rgba[i * 4 + 1] = data[i * 3 + 1];
            out_rgba[i * 4 + 2] = data[i * 3 + 2];
            out_rgba[i * 4 + 3] = 255;
        }
        Ok(required_rgba)
    } else if bpp == 32 {
        if data.len() < required_rgba {
            return Err(());
        }
        out_rgba[..required_rgba].copy_from_slice(&data[..required_rgba]);
        Ok(required_rgba)
    } else {
        Err(())
    }
}

/// Decodes an image into a bitmap that borrows the caller's buffer.
///
/// WHY this and not a function that allocates: the apps in this system have
/// no heap, so the pixels live in a buffer the caller owns and the bitmap
/// describes it.
pub fn decode_bitmap<'a>(
    data: &[u8],
    name: &[u8],
    out_rgba: &'a mut [u8],
) -> Result<Bitmap<'a>, ()> {
    let (w, h, len) = decode_image(data, name, out_rgba)?;
    let pixels: &'a [u8] = &out_rgba[..len];
    Ok(Bitmap {
        w,
        h,
        rgba: pixels,
    })
}

/// Decodes an image file by sniffing its magic header or file extension into RGBA.
pub fn decode_image(
    data: &[u8],
    name: &[u8],
    out_rgba: &mut [u8],
) -> Result<(u32, u32, usize), ()> {
    if name.ends_with(b".ppm") || (data.len() >= 2 && &data[0..2] == b"P6") {
        let (w, h, _) = decode_ppm_header(data)?;
        let bytes = decode_ppm(data, out_rgba)?;
        return Ok((w, h, bytes));
    }
    if name.ends_with(b".bmp") || (data.len() >= 2 && &data[0..2] == b"BM") {
        let (w, h, _, _) = decode_bmp_header(data)?;
        let bytes = decode_bmp(data, out_rgba)?;
        return Ok((w, h, bytes));
    }
    if name.ends_with(b".qoi") || (data.len() >= 4 && &data[0..4] == b"qoif") {
        let (w, h, _, _) = decode_qoi_header(data)?;
        let bytes = decode_qoi(data, out_rgba)?;
        return Ok((w, h, bytes));
    }
    if name.ends_with(b".raw") {
        let len = data.len();
        if len == 36 * 72 * 3 {
            let bytes = decode_raw(data, 36, 72, 24, out_rgba)?;
            return Ok((36, 72, bytes));
        } else if len == 36 * 72 * 4 {
            let bytes = decode_raw(data, 36, 72, 32, out_rgba)?;
            return Ok((36, 72, bytes));
        }
    }
    Err(())
}

