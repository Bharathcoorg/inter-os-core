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

    // A stream that stops before the last pixel is a truncated file. Returning
    // a short length would hand the caller a partial picture to draw as if it
    // were whole, which is the same lie as inventing the missing pixels.
    if out_pos != required_bytes {
        return Err(());
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

// ---------------------------------------------------------------------------
// PNG and GIF
// ---------------------------------------------------------------------------

/// The largest width or height, in pixels, that this module will decode.
///
/// WHY a limit exists: a PNG's dimensions come out of an untrusted header, and
/// a ten-byte file can name a picture of any size at all. The caller's buffer
/// already refuses anything that does not fit, but the decoder's own fixed
/// scratch (a row buffer pair) has to be sized for *some* maximum, so the
/// refusal is explicit and comes before any decoding work: 4096 is wider than
/// any surface this device paints, and it makes every scratch size below a
/// constant the linker can check.
pub const MAX_DECODE_DIMENSION: u32 = 4096;

/// The LZ77 history window every DEFLATE stream is allowed to reference.
const INFLATE_WINDOW: usize = 32 * 1024;

/// One scanline of the widest image, at four bytes per pixel.
const MAX_ROW_BYTES: usize = MAX_DECODE_DIMENSION as usize * 4;

/// The GIF LZW code table bound from the specification.
const LZW_MAX_CODES: usize = 4096;

/// Fixed scratch shared by the PNG and GIF decoders.
///
/// WHY static and not on the stack: an app in this system has a small stack,
/// and a 32 KiB inflate window plus two 16 KiB row buffers would not fit in
/// it. The buffers are uninitialised zeroed pages in `.bss`, so they cost
/// address space rather than image size, and they are sized by the constants
/// above rather than by anything a file says.
struct DecodeScratch {
    window: [u8; INFLATE_WINDOW],
    prev_row: [u8; MAX_ROW_BYTES],
    cur_row: [u8; MAX_ROW_BYTES],
    lzw_prefix: [u16; LZW_MAX_CODES],
    lzw_suffix: [u8; LZW_MAX_CODES],
    lzw_stack: [u8; LZW_MAX_CODES],
}

static mut DECODE_SCRATCH: DecodeScratch = DecodeScratch {
    window: [0; INFLATE_WINDOW],
    prev_row: [0; MAX_ROW_BYTES],
    cur_row: [0; MAX_ROW_BYTES],
    lzw_prefix: [0; LZW_MAX_CODES],
    lzw_suffix: [0; LZW_MAX_CODES],
    lzw_stack: [0; LZW_MAX_CODES],
};

/// Borrows the decoder scratch for the duration of one decode.
///
/// SAFETY: the scratch is sound because the callers are single-threaded and
/// non-reentrant. Every app in this system runs one event loop on one task and
/// calls `decode_image` from that loop, never from an interrupt handler or a
/// second thread, and neither `decode_png` nor `decode_gif` calls any function
/// that could decode again while it holds the borrow. The reference cannot
/// escape either: it is taken inside the decoder and dropped before it
/// returns. The unit tests, which the Rust test harness *does* run on several
/// threads, serialise themselves on a mutex so the same claim holds there.
fn decode_scratch() -> &'static mut DecodeScratch {
    unsafe { &mut *core::ptr::addr_of_mut!(DECODE_SCRATCH) }
}

/// CRC-32 as PNG defines it (polynomial 0xEDB88320, reflected, init/final XOR).
///
/// WHY implemented here rather than pulled from a crate: this crate is the
/// platform's `no_std` standard library, and PNG is the only thing in it that
/// needs a CRC. The bitwise form is slower than a table but the table would be
/// another kilobyte of image for a check that runs once per chunk.
fn png_crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// DEFLATE (RFC 1951) and its zlib framing (RFC 1950)
// ---------------------------------------------------------------------------

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];

const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];

/// A bit reader over a DEFLATE stream, least-significant bit first.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    cnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            cnt: 0,
        }
    }

    fn need(&mut self, n: u32) -> Result<(), ()> {
        while self.cnt < n {
            if self.pos >= self.data.len() {
                return Err(());
            }
            self.buf |= (self.data[self.pos] as u32) << self.cnt;
            self.pos += 1;
            self.cnt += 8;
        }
        Ok(())
    }

    fn bits(&mut self, n: u32) -> Result<u32, ()> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let value = self.buf & ((1u32 << n) - 1);
        self.buf >>= n;
        self.cnt -= n;
        Ok(value)
    }

    fn bit(&mut self) -> Result<u32, ()> {
        self.bits(1)
    }

    /// Discards the rest of the current byte, as stored blocks require.
    fn align(&mut self) {
        let drop = self.cnt % 8;
        self.buf >>= drop;
        self.cnt -= drop;
    }

    fn byte(&mut self) -> Result<u8, ()> {
        if self.cnt >= 8 {
            let value = (self.buf & 0xFF) as u8;
            self.buf >>= 8;
            self.cnt -= 8;
            Ok(value)
        } else if self.pos < self.data.len() {
            self.pos += 1;
            Ok(self.data[self.pos - 1])
        } else {
            Err(())
        }
    }

    /// The position of the next unconsumed byte in the input.
    fn stream_pos(&self) -> usize {
        self.pos - (self.cnt as usize / 8)
    }
}

/// A canonical Huffman decoding table.
///
/// WHY counts plus a sorted symbol list: DEFLATE gives only code *lengths*, and
/// the canonical construction sorts symbols by length then value. Decoding then
/// walks one bit at a time, comparing against the first code of each length,
/// which needs no lookup table and so costs no more fixed memory than the
/// alphabet itself.
struct Huff<const N: usize> {
    count: [u16; 16],
    symbol: [u16; N],
}

impl<const N: usize> Huff<N> {
    fn new() -> Self {
        Self {
            count: [0; 16],
            symbol: [0; N],
        }
    }

    fn build(&mut self, lengths: &[u8]) -> Result<(), ()> {
        if lengths.len() > N {
            return Err(());
        }
        self.count = [0; 16];
        for &length in lengths {
            if length > 15 {
                return Err(());
            }
            self.count[length as usize] += 1;
        }
        self.count[0] = 0;
        // Over-subscribed trees cannot be decoded. Incomplete trees are legal
        // for the distance alphabet and are left alone.
        let mut left: i32 = 1;
        for len in 1..16 {
            left <<= 1;
            left -= self.count[len] as i32;
            if left < 0 {
                return Err(());
            }
        }
        let mut offset = [0u16; 16];
        let mut total = 0u16;
        for len in 1..16 {
            offset[len] = total;
            total += self.count[len];
        }
        for (symbol, &length) in lengths.iter().enumerate() {
            if length != 0 {
                let slot = offset[length as usize] as usize;
                if slot >= N {
                    return Err(());
                }
                self.symbol[slot] = symbol as u16;
                offset[length as usize] += 1;
            }
        }
        Ok(())
    }

    fn decode(&self, bits: &mut BitReader<'_>) -> Result<u16, ()> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16usize {
            code |= bits.bit()? as i32;
            let count = self.count[len] as i32;
            if code - first < count {
                let slot = (index + code - first) as usize;
                if slot >= N {
                    return Err(());
                }
                return Ok(self.symbol[slot]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(())
    }
}

/// A streaming DEFLATE decoder that writes into the caller's scanline buffer.
///
/// WHY a pull decoder: the PNG decoder stays honest about memory by never
/// holding the whole filtered image. It asks for one scanline at a time and
/// this type keeps only the 32 KiB LZ77 window between requests, which is
/// exactly the history DEFLATE can reference.
struct Inflater<'a> {
    bits: BitReader<'a>,
    window: &'a mut [u8; INFLATE_WINDOW],
    /// Total bytes ever produced; also the ring write position, modulo window.
    wpos: usize,
    adler_a: u32,
    adler_b: u32,
    /// 0 = block header, 1 = stored bytes, 2 = Huffman symbols, 3 = match copy.
    mode: u8,
    bfinal: bool,
    final_done: bool,
    stored_left: usize,
    match_left: usize,
    match_dist: usize,
    lit: Huff<288>,
    dist: Huff<32>,
}

impl<'a> Inflater<'a> {
    fn new(input: &'a [u8], window: &'a mut [u8; INFLATE_WINDOW]) -> Self {
        Self {
            bits: BitReader::new(input),
            window,
            wpos: 0,
            adler_a: 1,
            adler_b: 0,
            mode: 0,
            bfinal: false,
            final_done: false,
            stored_left: 0,
            match_left: 0,
            match_dist: 0,
            lit: Huff::new(),
            dist: Huff::new(),
        }
    }

    fn adler(&self) -> u32 {
        (self.adler_b << 16) | self.adler_a
    }

    fn emit(&mut self, out: &mut [u8], written: &mut usize, byte: u8) -> Result<(), ()> {
        if *written >= out.len() {
            return Err(());
        }
        out[*written] = byte;
        *written += 1;
        self.window[self.wpos & (INFLATE_WINDOW - 1)] = byte;
        self.wpos += 1;
        // Adler-32 mod 65521, reduced per byte so nothing can overflow.
        self.adler_a += byte as u32;
        if self.adler_a >= 65521 {
            self.adler_a -= 65521;
        }
        self.adler_b += self.adler_a;
        if self.adler_b >= 65521 {
            self.adler_b -= 65521;
        }
        Ok(())
    }

    fn end_block(&mut self) {
        if self.bfinal {
            self.final_done = true;
        } else {
            self.mode = 0;
        }
    }

    fn fixed_tables(&mut self) -> Result<(), ()> {
        let mut lengths = [0u8; 288];
        for length in lengths.iter_mut().take(144) {
            *length = 8;
        }
        for length in lengths.iter_mut().take(256).skip(144) {
            *length = 9;
        }
        for length in lengths.iter_mut().take(280).skip(256) {
            *length = 7;
        }
        for length in lengths.iter_mut().skip(280) {
            *length = 8;
        }
        self.lit.build(&lengths)?;
        self.dist.build(&[5u8; 32])?;
        Ok(())
    }

    fn dynamic_tables(&mut self) -> Result<(), ()> {
        let hlit = self.bits.bits(5)? as usize + 257;
        let hdist = self.bits.bits(5)? as usize + 1;
        let hclen = self.bits.bits(4)? as usize + 4;
        if hlit > 286 || hdist > 32 {
            return Err(());
        }
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let mut code_lengths = [0u8; 19];
        for &slot in ORDER.iter().take(hclen) {
            code_lengths[slot] = self.bits.bits(3)? as u8;
        }
        let mut code_tree = Huff::<19>::new();
        code_tree.build(&code_lengths)?;

        let mut lengths = [0u8; 286 + 32];
        let total = hlit + hdist;
        let mut n = 0usize;
        while n < total {
            let symbol = code_tree.decode(&mut self.bits)?;
            match symbol {
                0..=15 => {
                    lengths[n] = symbol as u8;
                    n += 1;
                }
                16 => {
                    if n == 0 {
                        return Err(());
                    }
                    let previous = lengths[n - 1];
                    let repeat = 3 + self.bits.bits(2)? as usize;
                    if n + repeat > total {
                        return Err(());
                    }
                    for _ in 0..repeat {
                        lengths[n] = previous;
                        n += 1;
                    }
                }
                17 => {
                    let repeat = 3 + self.bits.bits(3)? as usize;
                    if n + repeat > total {
                        return Err(());
                    }
                    n += repeat;
                }
                18 => {
                    let repeat = 11 + self.bits.bits(7)? as usize;
                    if n + repeat > total {
                        return Err(());
                    }
                    n += repeat;
                }
                _ => return Err(()),
            }
        }
        self.lit.build(&lengths[..hlit])?;
        self.dist.build(&lengths[hlit..total])?;
        Ok(())
    }

    /// Advances the decoder by at least one symbol, writing output into `out`.
    fn step(&mut self, out: &mut [u8], written: &mut usize) -> Result<(), ()> {
        match self.mode {
            0 => {
                let bfinal = self.bits.bits(1)? != 0;
                let btype = self.bits.bits(2)?;
                self.bfinal = bfinal;
                match btype {
                    0 => {
                        self.bits.align();
                        let len = self.bits.bits(16)? as usize;
                        let nlen = self.bits.bits(16)? as usize;
                        if len ^ 0xFFFF != nlen {
                            return Err(());
                        }
                        self.stored_left = len;
                        self.mode = 1;
                        if len == 0 {
                            self.end_block();
                        }
                    }
                    1 => {
                        self.fixed_tables()?;
                        self.mode = 2;
                    }
                    2 => {
                        self.dynamic_tables()?;
                        self.mode = 2;
                    }
                    _ => return Err(()),
                }
            }
            1 => {
                if self.stored_left == 0 {
                    self.end_block();
                    return Ok(());
                }
                let available = out.len() - *written;
                if available == 0 {
                    return Err(());
                }
                let take = self.stored_left.min(available);
                for _ in 0..take {
                    let byte = self.bits.byte()?;
                    self.emit(out, written, byte)?;
                }
                self.stored_left -= take;
                if self.stored_left == 0 {
                    self.end_block();
                }
            }
            2 => {
                let symbol = self.lit.decode(&mut self.bits)?;
                if symbol < 256 {
                    self.emit(out, written, symbol as u8)?;
                } else if symbol == 256 {
                    self.end_block();
                } else if symbol <= 285 {
                    let index = (symbol - 257) as usize;
                    let length = LENGTH_BASE[index] as usize
                        + self.bits.bits(LENGTH_EXTRA[index] as u32)? as usize;
                    let dist_symbol = self.dist.decode(&mut self.bits)? as usize;
                    if dist_symbol >= 30 {
                        return Err(());
                    }
                    let distance = DIST_BASE[dist_symbol] as usize
                        + self.bits.bits(DIST_EXTRA[dist_symbol] as u32)? as usize;
                    if distance == 0 || distance > self.wpos {
                        return Err(());
                    }
                    self.match_left = length;
                    self.match_dist = distance;
                    self.mode = 3;
                } else {
                    return Err(());
                }
            }
            3 => {
                let available = out.len() - *written;
                if available == 0 {
                    return Err(());
                }
                let take = self.match_left.min(available);
                for _ in 0..take {
                    // Emitting one byte at a time is what makes overlapping
                    // matches (distance shorter than length) come out right.
                    let byte = self.window[(self.wpos - self.match_dist) & (INFLATE_WINDOW - 1)];
                    self.emit(out, written, byte)?;
                }
                self.match_left -= take;
                if self.match_left == 0 {
                    self.mode = 2;
                }
            }
            _ => return Err(()),
        }
        Ok(())
    }

    /// Fills `dst` completely, refusing a stream that ends early.
    fn fill(&mut self, dst: &mut [u8]) -> Result<(), ()> {
        let mut written = 0usize;
        while written < dst.len() {
            if self.final_done {
                return Err(());
            }
            self.step(dst, &mut written)?;
        }
        Ok(())
    }

    /// Consumes the end of the final block, refusing any extra output.
    fn finish(&mut self) -> Result<(), ()> {
        let mut empty: [u8; 0] = [];
        let mut written = 0usize;
        while !self.final_done {
            self.step(&mut empty, &mut written)?;
        }
        self.bits.align();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

#[derive(Clone, Copy)]
struct PngHeader {
    width: u32,
    height: u32,
    color_type: u8,
    interlace: u8,
}

/// Bytes per pixel of a PNG colour type at bit depth 8.
fn png_channels(color_type: u8) -> Result<usize, ()> {
    match color_type {
        0 | 3 => Ok(1),
        4 => Ok(2),
        2 => Ok(3),
        6 => Ok(4),
        _ => Err(()),
    }
}

/// Parses the signature and the IHDR chunk and refuses anything this decoder
/// cannot decode exactly.
fn png_parse_header(data: &[u8]) -> Result<PngHeader, ()> {
    if data.len() < 8 + 25 || data[..8] != PNG_SIGNATURE {
        return Err(());
    }
    let length = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
    if length != 13 || &data[12..16] != b"IHDR" {
        return Err(());
    }
    let want = u32::from_be_bytes([data[29], data[30], data[31], data[32]]);
    if png_crc32(&data[12..29]) != want {
        return Err(());
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    let bit_depth = data[24];
    let color_type = data[25];
    let compression = data[26];
    let filter = data[27];
    let interlace = data[28];
    if width == 0
        || height == 0
        || width > MAX_DECODE_DIMENSION
        || height > MAX_DECODE_DIMENSION
    {
        return Err(());
    }
    // Only 8-bit samples: 1/2/4-bit images would need sub-byte unpacking and
    // 16-bit images a sample width the rest of this module does not carry, and
    // guessing at either would produce a picture that is not the file's.
    if bit_depth != 8 {
        return Err(());
    }
    png_channels(color_type)?;
    if compression != 0 || filter != 0 || interlace > 1 {
        return Err(());
    }
    Ok(PngHeader {
        width,
        height,
        color_type,
        interlace,
    })
}

/// Returns the dimensions a PNG's header declares, without decoding pixels.
pub fn decode_png_header(data: &[u8]) -> Result<(u32, u32), ()> {
    let header = png_parse_header(data)?;
    Ok((header.width, header.height))
}

fn png_paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Reverses one PNG filter into `cur`, using the previous scanline.
fn png_unfilter(filter: u8, cur: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), ()> {
    if prev.len() < cur.len() {
        return Err(());
    }
    let len = cur.len();
    match filter {
        0 => {}
        1 => {
            for i in bpp..len {
                cur[i] = cur[i].wrapping_add(cur[i - bpp]);
            }
        }
        2 => {
            for i in 0..len {
                cur[i] = cur[i].wrapping_add(prev[i]);
            }
        }
        3 => {
            for i in 0..len {
                let left = if i >= bpp { cur[i - bpp] as u16 } else { 0 };
                let up = prev[i] as u16;
                cur[i] = cur[i].wrapping_add(((left + up) / 2) as u8);
            }
        }
        4 => {
            for i in 0..len {
                let left = if i >= bpp { cur[i - bpp] } else { 0 };
                let up = prev[i];
                let upper_left = if i >= bpp { prev[i - bpp] } else { 0 };
                cur[i] = cur[i].wrapping_add(png_paeth(left, up, upper_left));
            }
        }
        _ => return Err(()),
    }
    Ok(())
}

/// Expands `count` raw samples into RGBA at `x0 + i * dx`.
///
/// WHY the stride arguments: one function then serves both the progressive and
/// the Adam7 path, so the palette lookup, the tRNS comparison and the channel
/// unpacking cannot drift between two copies of the same loop.
#[allow(clippy::too_many_arguments)]
fn png_expand_row(
    color_type: u8,
    row: &[u8],
    count: usize,
    x0: usize,
    dx: usize,
    out_row: &mut [u8],
    palette: &[u8],
    palette_entries: usize,
    trns: &[u8],
    trns_entries: usize,
    gray_trns: Option<u16>,
    rgb_trns: Option<[u16; 3]>,
) -> Result<(), ()> {
    match color_type {
        0 => {
            if row.len() < count {
                return Err(());
            }
            for i in 0..count {
                let value = row[i];
                let offset = (x0 + i * dx) * 4;
                if offset + 4 > out_row.len() {
                    return Err(());
                }
                let alpha = match gray_trns {
                    Some(key) if key == value as u16 => 0,
                    _ => 255,
                };
                out_row[offset] = value;
                out_row[offset + 1] = value;
                out_row[offset + 2] = value;
                out_row[offset + 3] = alpha;
            }
        }
        2 => {
            if row.len() < count * 3 {
                return Err(());
            }
            for i in 0..count {
                let r = row[i * 3];
                let g = row[i * 3 + 1];
                let b = row[i * 3 + 2];
                let offset = (x0 + i * dx) * 4;
                if offset + 4 > out_row.len() {
                    return Err(());
                }
                let alpha = match rgb_trns {
                    Some(key) if key == [r as u16, g as u16, b as u16] => 0,
                    _ => 255,
                };
                out_row[offset] = r;
                out_row[offset + 1] = g;
                out_row[offset + 2] = b;
                out_row[offset + 3] = alpha;
            }
        }
        3 => {
            if row.len() < count {
                return Err(());
            }
            for i in 0..count {
                let index = row[i] as usize;
                if index >= palette_entries || index * 3 + 2 >= palette.len() {
                    return Err(());
                }
                let offset = (x0 + i * dx) * 4;
                if offset + 4 > out_row.len() {
                    return Err(());
                }
                out_row[offset] = palette[index * 3];
                out_row[offset + 1] = palette[index * 3 + 1];
                out_row[offset + 2] = palette[index * 3 + 2];
                // A palette tRNS entry is an alpha level, not only a flag.
                out_row[offset + 3] = if index < trns_entries { trns[index] } else { 255 };
            }
        }
        4 => {
            if row.len() < count * 2 {
                return Err(());
            }
            for i in 0..count {
                let value = row[i * 2];
                let offset = (x0 + i * dx) * 4;
                if offset + 4 > out_row.len() {
                    return Err(());
                }
                out_row[offset] = value;
                out_row[offset + 1] = value;
                out_row[offset + 2] = value;
                out_row[offset + 3] = row[i * 2 + 1];
            }
        }
        6 => {
            if row.len() < count * 4 {
                return Err(());
            }
            for i in 0..count {
                let offset = (x0 + i * dx) * 4;
                if offset + 4 > out_row.len() {
                    return Err(());
                }
                out_row[offset..offset + 4].copy_from_slice(&row[i * 4..i * 4 + 4]);
            }
        }
        _ => return Err(()),
    }
    Ok(())
}

/// Adam7 pass geometry: (x start, y start, x step, y step).
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

fn adam7_span(full: usize, start: usize, step: usize) -> usize {
    if full <= start {
        0
    } else {
        (full - start + step - 1) / step
    }
}

/// Decodes a PNG stream into an RGBA buffer.
///
/// Supports bit depth 8 with colour types 0, 2, 3, 4 and 6, palette
/// transparency and single-colour tRNS keys, progressive and Adam7 layouts.
/// Everything else -- other depths, other colour types, a missing palette,
/// a truncated or corrupt stream -- is refused rather than approximated.
pub fn decode_png(data: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    let header = png_parse_header(data)?;
    let channels = png_channels(header.color_type)?;
    let width = header.width as usize;
    let height = header.height as usize;
    let pixel_count = width.checked_mul(height).ok_or(())?;
    let required = pixel_count.checked_mul(4).ok_or(())?;
    if required > out_rgba.len() {
        return Err(());
    }

    let mut palette = [0u8; 768];
    let mut palette_entries = 0usize;
    let mut trns = [0u8; 256];
    let mut trns_entries = 0usize;
    let mut gray_trns: Option<u16> = None;
    let mut rgb_trns: Option<[u16; 3]> = None;
    let mut idat_start = 0usize;
    let mut idat_end = 0usize;
    let mut seen_idat = false;
    let mut idat_closed = false;
    let mut seen_iend = false;
    let mut seen_plte = false;

    // The signature and the fixed-size IHDR chunk have already been read.
    let mut pos = 8usize + 25;
    while pos + 8 <= data.len() {
        let length = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
            as usize;
        let kind = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
        let data_start = pos + 8;
        let data_end = data_start.checked_add(length).ok_or(())?;
        if data_end.checked_add(4).ok_or(())? > data.len() {
            return Err(());
        }
        let want = u32::from_be_bytes([
            data[data_end],
            data[data_end + 1],
            data[data_end + 2],
            data[data_end + 3],
        ]);
        if png_crc32(&data[pos + 4..data_end]) != want {
            return Err(());
        }
        let payload = &data[data_start..data_end];

        // IDAT chunks must be consecutive: only then is the compressed stream
        // one contiguous slice, which is what lets the inflater read it
        // without a concatenation buffer.
        if seen_idat && &kind != b"IDAT" {
            idat_closed = true;
        }

        match &kind {
            // The first chunk was already checked by png_parse_header.
            b"IHDR" => return Err(()),
            b"PLTE" => {
                if seen_idat
                    || seen_plte
                    || length == 0
                    || length % 3 != 0
                    || length > 768
                    || header.color_type == 0
                    || header.color_type == 4
                {
                    return Err(());
                }
                palette[..length].copy_from_slice(payload);
                palette_entries = length / 3;
                seen_plte = true;
            }
            b"tRNS" => {
                if seen_idat {
                    return Err(());
                }
                match header.color_type {
                    3 => {
                        if length > palette_entries {
                            return Err(());
                        }
                        trns[..length].copy_from_slice(payload);
                        trns_entries = length;
                    }
                    0 => {
                        if length != 2 {
                            return Err(());
                        }
                        gray_trns = Some(u16::from_be_bytes([payload[0], payload[1]]));
                    }
                    2 => {
                        if length != 6 {
                            return Err(());
                        }
                        rgb_trns = Some([
                            u16::from_be_bytes([payload[0], payload[1]]),
                            u16::from_be_bytes([payload[2], payload[3]]),
                            u16::from_be_bytes([payload[4], payload[5]]),
                        ]);
                    }
                    // tRNS is not defined for alpha-bearing colour types; it
                    // is ancillary, so it is ignored rather than trusted.
                    _ => {}
                }
            }
            b"IDAT" => {
                if idat_closed {
                    return Err(());
                }
                if !seen_idat {
                    idat_start = data_start;
                    seen_idat = true;
                }
                idat_end = data_end;
            }
            b"IEND" => {
                if length != 0 {
                    return Err(());
                }
                seen_iend = true;
                break;
            }
            _ => {
                // The high bit of the first type byte being clear marks a
                // *critical* chunk, and an unknown one means this decoder does
                // not know what the file requires of it.
                if kind[0] & 0x20 == 0 {
                    return Err(());
                }
            }
        }
        pos = data_end + 4;
    }

    if !seen_idat || !seen_iend {
        return Err(());
    }
    if header.color_type == 3 && !seen_plte {
        return Err(());
    }
    if idat_end < idat_start || idat_end - idat_start < 6 {
        return Err(());
    }
    let zdata = &data[idat_start..idat_end];
    let cmf = zdata[0];
    let flg = zdata[1];
    if cmf & 0x0F != 8 || (cmf >> 4) > 7 {
        return Err(());
    }
    if ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return Err(());
    }
    // A preset dictionary would have to be supplied out of band; a PNG never
    // carries one, so a stream that asks for one is not a PNG stream.
    if flg & 0x20 != 0 {
        return Err(());
    }

    let scratch = decode_scratch();
    let window = &mut scratch.window;
    let cur = &mut scratch.cur_row[..MAX_ROW_BYTES];
    let prev = &mut scratch.prev_row[..MAX_ROW_BYTES];
    let mut inflater = Inflater::new(&zdata[2..], window);

    let mut pass_index = 0usize;
    loop {
        let (x0, y0, dx, dy) = if header.interlace == 1 {
            if pass_index >= ADAM7.len() {
                break;
            }
            ADAM7[pass_index]
        } else {
            if pass_index >= 1 {
                break;
            }
            (0usize, 0usize, 1usize, 1usize)
        };
        pass_index += 1;

        let pass_w = adam7_span(width, x0, dx);
        let pass_h = adam7_span(height, y0, dy);
        if pass_w == 0 || pass_h == 0 {
            continue;
        }
        let pass_row = pass_w * channels;
        if pass_row > MAX_ROW_BYTES {
            return Err(());
        }
        // Each pass starts with the "previous row" defined as zeros.
        for byte in prev[..pass_row].iter_mut() {
            *byte = 0;
        }
        for pass_line in 0..pass_h {
            let y = y0 + pass_line * dy;
            let mut filter = [0u8; 1];
            inflater.fill(&mut filter)?;
            inflater.fill(&mut cur[..pass_row])?;
            png_unfilter(filter[0], &mut cur[..pass_row], &prev[..pass_row], channels)?;
            let row_start = y * width * 4;
            png_expand_row(
                header.color_type,
                &cur[..pass_row],
                pass_w,
                x0,
                dx,
                &mut out_rgba[row_start..row_start + width * 4],
                &palette,
                palette_entries,
                &trns,
                trns_entries,
                gray_trns,
                rgb_trns,
            )?;
            prev[..pass_row].copy_from_slice(&cur[..pass_row]);
        }
    }

    inflater.finish()?;
    let stream_end = 2 + inflater.bits.stream_pos();
    if stream_end + 4 != zdata.len() {
        return Err(());
    }
    let want = u32::from_be_bytes([
        zdata[stream_end],
        zdata[stream_end + 1],
        zdata[stream_end + 2],
        zdata[stream_end + 3],
    ]);
    if inflater.adler() != want {
        return Err(());
    }

    Ok((header.width, header.height, required))
}

// ---------------------------------------------------------------------------
// GIF
// ---------------------------------------------------------------------------

struct GifHeader {
    width: u32,
    height: u32,
    gct: Option<(usize, usize)>,
    pos: usize,
}

fn gif_parse_header(data: &[u8]) -> Result<GifHeader, ()> {
    if data.len() < 13 {
        return Err(());
    }
    if &data[..6] != b"GIF87a" && &data[..6] != b"GIF89a" {
        return Err(());
    }
    let width = u16::from_le_bytes([data[6], data[7]]) as u32;
    let height = u16::from_le_bytes([data[8], data[9]]) as u32;
    if width == 0 || height == 0 || width > MAX_DECODE_DIMENSION || height > MAX_DECODE_DIMENSION {
        return Err(());
    }
    let packed = data[10];
    let mut pos = 13usize;
    let gct = if packed & 0x80 != 0 {
        let entries = 2usize << (packed & 0x07);
        if pos + entries * 3 > data.len() {
            return Err(());
        }
        let offset = pos;
        pos += entries * 3;
        Some((offset, entries))
    } else {
        None
    };
    Ok(GifHeader {
        width,
        height,
        gct,
        pos,
    })
}

/// Returns the logical screen size a GIF's header declares.
///
/// WHY the logical screen and not the first frame: the decoder paints every
/// frame onto a canvas of this size, because a frame may be smaller than the
/// screen and its offset decides where the picture sits.
pub fn decode_gif_header(data: &[u8]) -> Result<(u32, u32), ()> {
    let header = gif_parse_header(data)?;
    Ok((header.width, header.height))
}

/// Walks the length-prefixed sub-block chain that carries GIF data.
struct GifSubBlocks<'a> {
    data: &'a [u8],
    pos: usize,
    left: usize,
    done: bool,
}

impl<'a> GifSubBlocks<'a> {
    fn new(data: &'a [u8], start: usize) -> Self {
        Self {
            data,
            pos: start,
            left: 0,
            done: false,
        }
    }

    fn next_byte(&mut self) -> Option<u8> {
        if self.done {
            return None;
        }
        while self.left == 0 {
            if self.pos >= self.data.len() {
                self.done = true;
                return None;
            }
            let length = self.data[self.pos] as usize;
            self.pos += 1;
            if length == 0 {
                // The zero-length sub-block is the terminator. A stream that
                // ends without an end code still stops here.
                self.done = true;
                return None;
            }
            self.left = length;
        }
        if self.pos >= self.data.len() {
            self.done = true;
            return None;
        }
        let byte = self.data[self.pos];
        self.pos += 1;
        self.left -= 1;
        Some(byte)
    }
}

fn gif_skip_sub_blocks(data: &[u8], mut pos: usize) -> Result<usize, ()> {
    loop {
        if pos >= data.len() {
            return Err(());
        }
        let length = data[pos] as usize;
        pos += 1;
        if length == 0 {
            return Ok(pos);
        }
        if pos + length > data.len() {
            return Err(());
        }
        pos += length;
    }
}

/// Reads GIF LZW codes, least-significant bit first, from the sub-block chain.
struct GifBits<'a> {
    blocks: GifSubBlocks<'a>,
    buf: u32,
    cnt: u32,
}

impl<'a> GifBits<'a> {
    fn new(data: &'a [u8], start: usize) -> Self {
        Self {
            blocks: GifSubBlocks::new(data, start),
            buf: 0,
            cnt: 0,
        }
    }

    fn read(&mut self, n: u32) -> Option<u16> {
        if n == 0 {
            return Some(0);
        }
        while self.cnt < n {
            let byte = self.blocks.next_byte()?;
            self.buf |= (byte as u32) << self.cnt;
            self.cnt += 8;
        }
        let value = (self.buf & ((1u32 << n) - 1)) as u16;
        self.buf >>= n;
        self.cnt -= n;
        Some(value)
    }
}

/// Maps a frame's row index to its screen row under GIF interlacing.
fn gif_row_y(row: usize, height: usize, interlace: bool) -> usize {
    if !interlace {
        return row;
    }
    const PASSES: [(usize, usize); 4] = [(0, 8), (4, 8), (2, 4), (1, 2)];
    let mut remaining = row;
    for (start, step) in PASSES {
        let rows = if height > start {
            (height - start + step - 1) / step
        } else {
            0
        };
        if remaining < rows {
            return start + remaining * step;
        }
        remaining -= rows;
    }
    row
}

/// Places decoded palette indices into the frame's rectangle of the canvas.
struct GifSink<'a> {
    out: &'a mut [u8],
    palette: &'a [u8],
    palette_entries: usize,
    transparent: Option<u8>,
    screen_w: usize,
    left: usize,
    top: usize,
    frame_w: usize,
    frame_h: usize,
    interlace: bool,
    col: usize,
    row: usize,
    y: usize,
}

impl GifSink<'_> {
    fn put(&mut self, index: u8) -> Result<(), ()> {
        if index as usize >= self.palette_entries || self.row >= self.frame_h {
            return Err(());
        }
        let offset = ((self.top + self.y) * self.screen_w + self.left + self.col) * 4;
        if offset + 4 > self.out.len() {
            return Err(());
        }
        let entry = index as usize * 3;
        self.out[offset] = self.palette[entry];
        self.out[offset + 1] = self.palette[entry + 1];
        self.out[offset + 2] = self.palette[entry + 2];
        // A transparent index keeps its palette colour but is not painted:
        // alpha zero is what the compositor needs to skip it.
        self.out[offset + 3] = if self.transparent == Some(index) { 0 } else { 255 };
        self.col += 1;
        if self.col == self.frame_w {
            self.col = 0;
            self.row += 1;
            if self.row < self.frame_h {
                self.y = gif_row_y(self.row, self.frame_h, self.interlace);
            }
        }
        Ok(())
    }
}

/// Decodes the first frame of a GIF into an RGBA canvas.
///
/// GIF LZW is the variable-width-code variant: codes start at
/// `min_code_size + 1` bits, the table grows by one after each code, and the
/// code width grows when the next free code would no longer fit. `CLEAR` and
/// `END` are the two codes above the literal alphabet. The table is bounded at
/// 4096 entries, at which point the encoder must send `CLEAR`.
#[allow(clippy::too_many_arguments)]
fn gif_lzw_decode(
    data: &[u8],
    start: usize,
    min_code_size: u8,
    frame_w: usize,
    frame_h: usize,
    interlace: bool,
    screen_w: usize,
    left: usize,
    top: usize,
    out: &mut [u8],
    palette: &[u8],
    palette_entries: usize,
    transparent: Option<u8>,
) -> Result<(), ()> {
    // The spec allows 2..=8; a code size of 1 has no CLEAR code below 4.
    if !(2..=8).contains(&min_code_size) {
        return Err(());
    }
    const NO_PARENT: u16 = 0xFFFF;
    let clear = 1u16 << min_code_size;
    let end = clear + 1;

    let scratch = decode_scratch();
    let prefix = &mut scratch.lzw_prefix;
    let suffix = &mut scratch.lzw_suffix;
    let stack = &mut scratch.lzw_stack;
    for code in 0..clear as usize {
        prefix[code] = NO_PARENT;
        suffix[code] = code as u8;
    }

    let mut next = end + 1;
    let mut code_size = min_code_size as u32 + 1;
    let mut prev: Option<u16> = None;
    let mut prev_first = 0u8;
    let mut sink = GifSink {
        out,
        palette,
        palette_entries,
        transparent,
        screen_w,
        left,
        top,
        frame_w,
        frame_h,
        interlace,
        col: 0,
        row: 0,
        y: gif_row_y(0, frame_h, interlace),
    };

    let mut bits = GifBits::new(data, start);
    loop {
        let code = match bits.read(code_size) {
            Some(code) => code,
            None => break,
        };
        if code == clear {
            next = end + 1;
            code_size = min_code_size as u32 + 1;
            prev = None;
            continue;
        }
        if code == end {
            break;
        }
        if code < next {
            // A code that is already in the table: emit its string, then add
            // prev + first_byte(this string) as the next entry.
            let mut n = 0usize;
            let mut c = code;
            while c != NO_PARENT {
                if n >= LZW_MAX_CODES {
                    return Err(());
                }
                stack[n] = suffix[c as usize];
                n += 1;
                c = prefix[c as usize];
            }
            if n == 0 {
                return Err(());
            }
            for i in (0..n).rev() {
                sink.put(stack[i])?;
            }
            let first = stack[n - 1];
            if let Some(previous) = prev {
                if (next as usize) < LZW_MAX_CODES {
                    prefix[next as usize] = previous;
                    suffix[next as usize] = first;
                    next += 1;
                    if next == (1u16 << code_size) && code_size < 12 {
                        code_size += 1;
                    }
                }
            }
            prev = Some(code);
            prev_first = first;
        } else if code == next {
            // The one code whose string is not in the table yet: it is
            // prev's string followed by prev's own first byte.
            let previous = prev.ok_or(())?;
            let mut n = 0usize;
            let mut c = previous;
            while c != NO_PARENT {
                if n >= LZW_MAX_CODES {
                    return Err(());
                }
                stack[n] = suffix[c as usize];
                n += 1;
                c = prefix[c as usize];
            }
            if n == 0 {
                return Err(());
            }
            for i in (0..n).rev() {
                sink.put(stack[i])?;
            }
            sink.put(prev_first)?;
            if (next as usize) < LZW_MAX_CODES {
                prefix[next as usize] = previous;
                suffix[next as usize] = prev_first;
                next += 1;
                if next == (1u16 << code_size) && code_size < 12 {
                    code_size += 1;
                }
            }
            prev = Some(code);
        } else {
            return Err(());
        }
    }

    // Every pixel of the frame has to have been produced: a stream that stops
    // early is a truncated file, not a frame with undefined corners.
    if sink.row != frame_h || sink.col != 0 {
        return Err(());
    }
    Ok(())
}

/// Decodes the first frame of a GIF87a/GIF89a stream into RGBA.
///
/// The canvas is the logical screen, a frame smaller than it is placed at its
/// own offset, and anything the frame does not cover stays transparent black.
/// There is no animation API yet: later frames are not read.
pub fn decode_gif(data: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    let header = gif_parse_header(data)?;
    let screen_w = header.width as usize;
    let screen_h = header.height as usize;
    let required = screen_w
        .checked_mul(screen_h)
        .ok_or(())?
        .checked_mul(4)
        .ok_or(())?;
    if required > out_rgba.len() {
        return Err(());
    }
    for byte in out_rgba[..required].iter_mut() {
        *byte = 0;
    }

    let mut pos = header.pos;
    let mut transparent: Option<u8> = None;
    loop {
        if pos >= data.len() {
            return Err(());
        }
        match data[pos] {
            0x3B => return Err(()), // trailer before any frame
            0x00 => pos += 1,
            0x21 => {
                if pos + 2 > data.len() {
                    return Err(());
                }
                let label = data[pos + 1];
                pos += 2;
                if label == 0xF9 {
                    // Graphics Control Extension: 0x21 0xF9 04 <packed>
                    // <delay lo> <delay hi> <transparent index> 00.
                    if pos + 6 > data.len() || data[pos] != 4 || data[pos + 5] != 0 {
                        return Err(());
                    }
                    transparent = if data[pos + 1] & 1 != 0 {
                        Some(data[pos + 4])
                    } else {
                        None
                    };
                    pos += 6;
                } else {
                    pos = gif_skip_sub_blocks(data, pos)?;
                }
            }
            0x2C => {
                if pos + 10 > data.len() {
                    return Err(());
                }
                let left = u16::from_le_bytes([data[pos + 1], data[pos + 2]]) as usize;
                let top = u16::from_le_bytes([data[pos + 3], data[pos + 4]]) as usize;
                let frame_w = u16::from_le_bytes([data[pos + 5], data[pos + 6]]) as usize;
                let frame_h = u16::from_le_bytes([data[pos + 7], data[pos + 8]]) as usize;
                let packed = data[pos + 9];
                pos += 10;
                if frame_w == 0
                    || frame_h == 0
                    || frame_w > MAX_DECODE_DIMENSION as usize
                    || frame_h > MAX_DECODE_DIMENSION as usize
                {
                    return Err(());
                }
                if left + frame_w > screen_w || top + frame_h > screen_h {
                    return Err(());
                }
                let (palette_offset, palette_entries) = if packed & 0x80 != 0 {
                    let entries = 2usize << (packed & 0x07);
                    if pos + entries * 3 > data.len() {
                        return Err(());
                    }
                    let offset = pos;
                    pos += entries * 3;
                    (offset, entries)
                } else {
                    match header.gct {
                        Some((offset, entries)) => (offset, entries),
                        None => return Err(()),
                    }
                };
                if pos >= data.len() {
                    return Err(());
                }
                let min_code_size = data[pos];
                pos += 1;
                let palette = &data[palette_offset..palette_offset + palette_entries * 3];
                gif_lzw_decode(
                    data,
                    pos,
                    min_code_size,
                    frame_w,
                    frame_h,
                    packed & 0x40 != 0,
                    screen_w,
                    left,
                    top,
                    &mut out_rgba[..required],
                    palette,
                    palette_entries,
                    transparent,
                )?;
                return Ok((header.width, header.height, required));
            }
            _ => return Err(()),
        }
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
    // WHY PNG and GIF are matched last: their detection is by signature, which
    // no other format here can imitate, and putting them after the suffix
    // checks above keeps every existing extension-to-decoder decision exactly
    // as it was for files that really are PPM, BMP, QOI or RAW.
    if (data.len() >= 8 && data[..8] == PNG_SIGNATURE) || name_ends_with(name, b".png") {
        return decode_png(data, out_rgba);
    }
    if (data.len() >= 6 && (&data[..6] == b"GIF87a" || &data[..6] == b"GIF89a"))
        || name_ends_with(name, b".gif")
    {
        return decode_gif(data, out_rgba);
    }
    Err(())
}

/// Whether `name` ends with `suffix`, ignoring ASCII case and requiring at
/// least one character of base name before it.
fn name_ends_with(name: &[u8], suffix: &[u8]) -> bool {
    name.len() > suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};
    use std::vec::Vec;

    /// The decoders share one static scratch buffer, so the tests that use it
    /// are serialised. In the apps it is sound because they are
    /// single-threaded; see `decode_scratch`.
    static SCRATCH_LOCK: Mutex<()> = Mutex::new(());

    fn scratch_guard() -> MutexGuard<'static, ()> {
        SCRATCH_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // -- PNG builders ------------------------------------------------------

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        let crc = png_crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    fn adler32(data: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// A zlib stream made of stored DEFLATE blocks: valid, if not compact.
    fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x78, 0x01]);
        if raw.is_empty() {
            out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
        } else {
            let mut offset = 0usize;
            while offset < raw.len() {
                let take = (raw.len() - offset).min(65535);
                let last = offset + take == raw.len();
                out.push(if last { 1 } else { 0 });
                let len = take as u16;
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&(!len).to_le_bytes());
                out.extend_from_slice(&raw[offset..offset + take]);
                offset += take;
            }
        }
        out.extend_from_slice(&adler32(raw).to_be_bytes());
        out
    }

    fn build_png_with_zdata(
        width: u32,
        height: u32,
        color_type: u8,
        interlace: u8,
        plte: Option<&[u8]>,
        trns: Option<&[u8]>,
        zdata: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, color_type, 0, 0, interlace]);
        chunk(&mut out, b"IHDR", &ihdr);
        if let Some(palette) = plte {
            chunk(&mut out, b"PLTE", palette);
        }
        if let Some(alpha) = trns {
            chunk(&mut out, b"tRNS", alpha);
        }
        chunk(&mut out, b"IDAT", zdata);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    fn build_png(
        width: u32,
        height: u32,
        color_type: u8,
        interlace: u8,
        plte: Option<&[u8]>,
        trns: Option<&[u8]>,
        filtered: &[u8],
    ) -> Vec<u8> {
        build_png_with_zdata(
            width,
            height,
            color_type,
            interlace,
            plte,
            trns,
            &zlib_stored(filtered),
        )
    }

    fn rewrite_ihdr_crc(png: &mut [u8]) {
        let crc = png_crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
    }

    fn test_paeth(a: u8, b: u8, c: u8) -> u8 {
        let p = a as i32 + b as i32 - c as i32;
        let pa = (p - a as i32).abs();
        let pb = (p - b as i32).abs();
        let pc = (p - c as i32).abs();
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    }

    /// Filters already-unfiltered rows as an encoder would.
    fn filter_stream(rows: &[Vec<u8>], filters: &[u8], bpp: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            let filter = filters[y];
            out.push(filter);
            let prev: &[u8] = if y == 0 { &[] } else { &rows[y - 1] };
            for i in 0..row.len() {
                let left = if i >= bpp { row[i - bpp] } else { 0 };
                let up = if prev.is_empty() { 0 } else { prev[i] };
                let upper_left = if !prev.is_empty() && i >= bpp {
                    prev[i - bpp]
                } else {
                    0
                };
                let predictor = match filter {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((left as u16 + up as u16) / 2) as u8,
                    4 => test_paeth(left, up, upper_left),
                    _ => panic!("test asked for an unknown filter"),
                };
                out.push(row[i].wrapping_sub(predictor));
            }
        }
        out
    }

    /// Lays an RGBA image out in Adam7 pass order, filter 0 on every row.
    fn adam7_stream(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
        const PASSES: [(usize, usize, usize, usize); 7] = [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ];
        let mut out = Vec::new();
        for (x0, y0, dx, dy) in PASSES {
            if width <= x0 || height <= y0 {
                continue;
            }
            let mut y = y0;
            while y < height {
                out.push(0);
                let mut x = x0;
                while x < width {
                    let offset = (y * width + x) * 4;
                    out.extend_from_slice(&rgba[offset..offset + 4]);
                    x += dx;
                }
                y += dy;
            }
        }
        out
    }

    // -- GIF builders ------------------------------------------------------

    const GIF_PALETTE: [[u8; 3]; 4] = [[0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255]];

    fn push_lzw_code(out: &mut Vec<u8>, acc: &mut u32, cnt: &mut u32, code: u16, size: u32) {
        *acc |= (code as u32) << *cnt;
        *cnt += size;
        while *cnt >= 8 {
            out.push((*acc & 0xFF) as u8);
            *acc >>= 8;
            *cnt -= 8;
        }
    }

    /// Encodes indices as literals, sending CLEAR before each so the code width
    /// never grows and the encoder needs no code table of its own.
    fn gif_lzw_literals(indices: &[u8], min_code_size: u8) -> Vec<u8> {
        let clear = 1u16 << min_code_size;
        let end = clear + 1;
        let size = min_code_size as u32 + 1;
        let mut out = Vec::new();
        let mut acc: u32 = 0;
        let mut cnt: u32 = 0;
        for &index in indices {
            push_lzw_code(&mut out, &mut acc, &mut cnt, clear, size);
            push_lzw_code(&mut out, &mut acc, &mut cnt, index as u16, size);
        }
        push_lzw_code(&mut out, &mut acc, &mut cnt, end, size);
        if cnt > 0 {
            out.push((acc & 0xFF) as u8);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn build_gif(
        screen_w: u16,
        screen_h: u16,
        transparent: Option<u8>,
        left: u16,
        top: u16,
        frame_w: u16,
        frame_h: u16,
        indices: &[u8],
        interlace: bool,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&screen_w.to_le_bytes());
        out.extend_from_slice(&screen_h.to_le_bytes());
        out.push(0x80 | 0x01); // global colour table, 2^(1+1) = 4 entries
        out.push(0);
        out.push(0);
        for entry in GIF_PALETTE {
            out.extend_from_slice(&entry);
        }
        if let Some(index) = transparent {
            out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, index, 0x00]);
        }
        out.push(0x2C);
        out.extend_from_slice(&left.to_le_bytes());
        out.extend_from_slice(&top.to_le_bytes());
        out.extend_from_slice(&frame_w.to_le_bytes());
        out.extend_from_slice(&frame_h.to_le_bytes());
        out.push(if interlace { 0x40 } else { 0x00 });
        out.push(2); // LZW minimum code size
        let lzw = gif_lzw_literals(indices, 2);
        for block in lzw.chunks(255) {
            out.push(block.len() as u8);
            out.extend_from_slice(block);
        }
        out.push(0);
        out.push(0x3B);
        out
    }

    // -- fixtures ----------------------------------------------------------
    //
    // Both were produced by a standard zlib compressor, so they pin the
    // inflater against streams this decoder did not build. The first uses a
    // fixed-Huffman block, the second a dynamic-Huffman block.

    const PNG_FIXED_HUFFMAN_RGB_2X2: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8,
        2, 0, 0, 0, 253, 212, 154, 115, 0, 0, 0, 22, 73, 68, 65, 84, 120, 218, 99, 224, 18, 145,
        211, 48, 178, 97, 112, 11, 136, 74, 201, 171, 0, 0, 15, 24, 3, 13, 185, 185, 232, 100, 0,
        0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    const PNG_DYNAMIC_HUFFMAN_RGBA_6X6: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 6, 0, 0, 0, 6, 8,
        6, 0, 0, 0, 224, 204, 239, 72, 0, 0, 0, 94, 73, 68, 65, 84, 120, 218, 13, 201, 145, 2, 4,
        65, 16, 67, 193, 198, 195, 224, 98, 112, 48, 56, 24, 92, 236, 255, 255, 153, 119, 91, 90,
        51, 51, 104, 126, 120, 68, 230, 161, 99, 118, 14, 51, 250, 33, 9, 235, 33, 50, 213, 97,
        149, 47, 44, 228, 7, 219, 196, 135, 58, 172, 239, 23, 121, 80, 140, 115, 72, 66, 115,
        217, 244, 139, 26, 245, 224, 134, 244, 210, 150, 237, 251, 197, 30, 180, 193, 123, 201,
        150, 238, 203, 238, 242, 7, 41, 170, 52, 189, 84, 151, 184, 176, 0, 0, 0, 0, 73, 69, 78,
        68, 174, 66, 96, 130,
    ];

    // -- PNG tests ---------------------------------------------------------

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(png_crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(png_crc32(b""), 0);
    }

    #[test]
    fn png_gray_decodes_one_and_two_pixel_images() {
        let _guard = scratch_guard();
        let png = build_png(1, 1, 0, 0, None, None, &[0, 0x7F]);
        let mut out = [0u8; 4];
        assert_eq!(decode_png(&png, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[0x7F, 0x7F, 0x7F, 255]);

        let png = build_png(2, 2, 0, 0, None, None, &[0, 1, 2, 0, 3, 4]);
        let mut out = [0u8; 16];
        assert_eq!(decode_png(&png, &mut out), Ok((2, 2, 16)));
        assert_eq!(&out[..8], &[1, 1, 1, 255, 2, 2, 2, 255]);
        assert_eq!(&out[8..], &[3, 3, 3, 255, 4, 4, 4, 255]);
    }

    #[test]
    fn png_rgb_decodes_one_and_two_pixel_images() {
        let _guard = scratch_guard();
        let png = build_png(1, 1, 2, 0, None, None, &[0, 5, 6, 7]);
        let mut out = [0u8; 4];
        assert_eq!(decode_png(&png, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[5, 6, 7, 255]);

        let png = build_png(
            2,
            2,
            2,
            0,
            None,
            None,
            &[0, 10, 20, 30, 40, 50, 60, 0, 70, 80, 90, 100, 110, 120],
        );
        let mut out = [0u8; 16];
        assert_eq!(decode_png(&png, &mut out), Ok((2, 2, 16)));
        assert_eq!(
            &out,
            &[
                10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255
            ]
        );
    }

    #[test]
    fn png_rgba_decodes_one_and_two_pixel_images() {
        let _guard = scratch_guard();
        let png = build_png(1, 1, 6, 0, None, None, &[0, 1, 2, 3, 4]);
        let mut out = [0u8; 4];
        assert_eq!(decode_png(&png, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[1, 2, 3, 4]);

        let png = build_png(
            2,
            2,
            6,
            0,
            None,
            None,
            &[
                0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 9, 10, 11, 12, 13, 14, 15, 16,
            ],
        );
        let mut out = [0u8; 16];
        assert_eq!(decode_png(&png, &mut out), Ok((2, 2, 16)));
        assert_eq!(&out, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    }

    #[test]
    fn png_gray_alpha_decodes() {
        let _guard = scratch_guard();
        let png = build_png(2, 1, 4, 0, None, None, &[0, 0x40, 0x80, 0xC0, 0x10]);
        let mut out = [0u8; 8];
        assert_eq!(decode_png(&png, &mut out), Ok((2, 1, 8)));
        assert_eq!(&out, &[0x40, 0x40, 0x40, 0x80, 0xC0, 0xC0, 0xC0, 0x10]);
    }

    #[test]
    fn png_palette_uses_plte_and_trns_alpha() {
        let _guard = scratch_guard();
        let palette = [10u8, 20, 30, 40, 50, 60, 70, 80, 90];
        let trns = [128u8];
        // 2x2, indices 0,1 / 2,1, each row prefixed with filter 0.
        let filtered = [0u8, 0, 1, 0, 2, 1];
        let png = build_png(2, 2, 3, 0, Some(&palette), Some(&trns), &filtered);
        let mut out = [0u8; 16];
        assert_eq!(decode_png(&png, &mut out), Ok((2, 2, 16)));
        assert_eq!(&out[0..4], &[10, 20, 30, 128]);
        assert_eq!(&out[4..8], &[40, 50, 60, 255]);
        assert_eq!(&out[8..12], &[70, 80, 90, 255]);
        assert_eq!(&out[12..16], &[40, 50, 60, 255]);
    }

    #[test]
    fn png_gray_and_rgb_transparent_keys_become_alpha_zero() {
        let _guard = scratch_guard();
        let png = build_png(2, 1, 0, 0, None, Some(&[0, 5]), &[0, 5, 6]);
        let mut out = [0u8; 8];
        assert_eq!(decode_png(&png, &mut out), Ok((2, 1, 8)));
        assert_eq!(&out, &[5, 5, 5, 0, 6, 6, 6, 255]);

        let png = build_png(
            1,
            1,
            2,
            0,
            None,
            Some(&[0, 1, 0, 2, 0, 3]),
            &[0, 1, 2, 3],
        );
        let mut out = [0u8; 4];
        assert_eq!(decode_png(&png, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[1, 2, 3, 0]);
    }

    #[test]
    fn png_unfilters_every_filter_type() {
        let _guard = scratch_guard();
        let rows: [Vec<u8>; 5] = [
            Vec::from([10u8, 20, 30, 40]),
            Vec::from([15u8, 25, 35, 45]),
            Vec::from([200u8, 100, 50, 25]),
            Vec::from([1u8, 2, 3, 4]),
            Vec::from([255u8, 0, 128, 64]),
        ];
        let filters = [0u8, 1, 2, 3, 4];
        let stream = filter_stream(&rows, &filters, 1);
        let png = build_png(4, 5, 0, 0, None, None, &stream);
        let mut out = [0u8; 4 * 5 * 4];
        assert_eq!(decode_png(&png, &mut out), Ok((4, 5, 80)));
        for y in 0..5 {
            for x in 0..4 {
                let offset = (y * 4 + x) * 4;
                let value = rows[y][x];
                assert_eq!(
                    &out[offset..offset + 4],
                    &[value, value, value, 255],
                    "row {} filter {} pixel {}",
                    y,
                    filters[y],
                    x
                );
            }
        }
    }

    #[test]
    fn png_fixed_huffman_fixture_decodes() {
        let _guard = scratch_guard();
        let mut out = [0u8; 16];
        assert_eq!(
            decode_png(PNG_FIXED_HUFFMAN_RGB_2X2, &mut out),
            Ok((2, 2, 16))
        );
        assert_eq!(
            &out,
            &[
                10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255
            ]
        );
    }

    #[test]
    fn png_dynamic_huffman_fixture_decodes() {
        let _guard = scratch_guard();
        let mut out = [0u8; 6 * 6 * 4];
        assert_eq!(
            decode_png(PNG_DYNAMIC_HUFFMAN_RGBA_6X6, &mut out),
            Ok((6, 6, 144))
        );
        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out[4..8], &[16, 0, 8, 255]);
        assert_eq!(&out[5 * 4 + 5 * 24..5 * 4 + 5 * 24 + 4], &[80, 80, 80, 255]);
        assert_eq!(&out[140..144], &[80, 80, 80, 255]);
    }

    #[test]
    fn png_adam7_interlaced_is_decoded() {
        let _guard = scratch_guard();
        let width = 5usize;
        let height = 5usize;
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&[(x * 20) as u8, (y * 30) as u8, (x * y) as u8, 255]);
            }
        }
        let stream = adam7_stream(width, height, &rgba);
        let png = build_png(5, 5, 6, 1, None, None, &stream);
        let mut out = [0u8; 5 * 5 * 4];
        assert_eq!(decode_png(&png, &mut out), Ok((5, 5, 100)));
        assert_eq!(&out[..], &rgba[..]);
    }

    #[test]
    fn qoi_stream_that_stops_early_is_refused() {
        // The header promises two pixels; the stream carries one and then the
        // end marker. A decoder that returned the one pixel would draw a
        // picture the file never held.
        let truncated = [
            b'q', b'o', b'i', b'f',
            0, 0, 0, 2, // width = 2
            0, 0, 0, 1, // height = 1
            4, 0, // channels, colorspace
            0xFE, 255, 0, 0, // QOI_OP_RGB
            0, 0, 0, 0, 0, 0, 0, 1, // end marker
        ];
        let mut out = [0u8; 2 * 4];
        assert_eq!(decode_qoi(&truncated, &mut out), Err(()));

        let full = [
            b'q', b'o', b'i', b'f',
            0, 0, 0, 2,
            0, 0, 0, 1,
            4, 0,
            0xFE, 255, 0, 0,
            0xFE, 0, 255, 0,
            0, 0, 0, 0, 0, 0, 0, 1,
        ];
        assert_eq!(decode_qoi(&full, &mut out), Ok(8));
        assert_eq!(&out[4..8], &[0, 255, 0, 255]);
    }

    #[test]
    fn png_header_reports_dimensions_only_for_a_decodable_header() {
        let png = PNG_FIXED_HUFFMAN_RGB_2X2;
        assert_eq!(decode_png_header(png), Ok((2, 2)));
        assert!(decode_png_header(&png[..10]).is_err());
        assert!(decode_png_header(b"not a png at all").is_err());
    }

    #[test]
    fn png_truncated_signature_and_ihdr_are_refused() {
        let _guard = scratch_guard();
        let png = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        let mut out = [0u8; 4];
        assert!(decode_png(&png[..7], &mut out).is_err());
        assert!(decode_png(&png[..28], &mut out).is_err());
        assert!(decode_png(&[], &mut out).is_err());

        let mut bad_signature = png.clone();
        bad_signature[1] = b'J';
        assert!(decode_png(&bad_signature, &mut out).is_err());
    }

    #[test]
    fn png_truncated_idat_is_refused() {
        let _guard = scratch_guard();
        let zdata = zlib_stored(&[0, 7]);
        let cut = &zdata[..zdata.len() - 3];
        let png = build_png_with_zdata(1, 1, 0, 0, None, None, cut);
        let mut out = [0u8; 4];
        assert!(decode_png(&png, &mut out).is_err());

        // A chunk that ends before any compressed data at all is also refused.
        let png = build_png_with_zdata(1, 1, 0, 0, None, None, &[0x78, 0x01]);
        assert!(decode_png(&png, &mut out).is_err());
    }

    #[test]
    fn png_corrupt_chunk_crc_is_refused() {
        let _guard = scratch_guard();
        let mut png = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        png[30] ^= 0xFF; // inside the IHDR CRC
        let mut out = [0u8; 4];
        assert!(decode_png(&png, &mut out).is_err());

        let mut png = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        png[41] ^= 0x01; // inside the IDAT payload
        assert!(decode_png(&png, &mut out).is_err());
    }

    #[test]
    fn png_bad_adler32_is_refused() {
        let _guard = scratch_guard();
        let mut zdata = zlib_stored(&[0, 7]);
        let last = zdata.len() - 1;
        zdata[last] ^= 0xFF;
        let png = build_png_with_zdata(1, 1, 0, 0, None, None, &zdata);
        let mut out = [0u8; 4];
        assert!(decode_png(&png, &mut out).is_err());

        // A preset-dictionary request is not a PNG stream either.
        let mut zdata = zlib_stored(&[0, 7]);
        zdata[1] |= 0x20;
        let png = build_png_with_zdata(1, 1, 0, 0, None, None, &zdata);
        assert!(decode_png(&png, &mut out).is_err());
    }

    #[test]
    fn png_unsupported_headers_are_refused() {
        let _guard = scratch_guard();
        let mut out = [0u8; 4];

        let mut sixteen_bit = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        sixteen_bit[24] = 16; // bit depth
        rewrite_ihdr_crc(&mut sixteen_bit);
        assert!(decode_png(&sixteen_bit, &mut out).is_err());

        let mut bad_type = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        bad_type[25] = 5; // no such PNG colour type
        rewrite_ihdr_crc(&mut bad_type);
        assert!(decode_png(&bad_type, &mut out).is_err());

        let zero_width = build_png(0, 1, 0, 0, None, None, &[0]);
        assert!(decode_png(&zero_width, &mut out).is_err());

        let huge_width = build_png(MAX_DECODE_DIMENSION + 1, 1, 0, 0, None, None, &[0]);
        assert!(decode_png(&huge_width, &mut out).is_err());
    }

    #[test]
    fn png_unknown_critical_chunk_is_refused_but_ancillary_is_ignored() {
        let _guard = scratch_guard();
        let mut out = [0u8; 4];

        let mut critical = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        let mut extra = Vec::new();
        chunk(&mut extra, b"ZZZZ", b"x");
        critical.splice(33..33, extra);
        assert!(decode_png(&critical, &mut out).is_err());

        let mut ancillary = build_png(1, 1, 0, 0, None, None, &[0, 7]);
        let mut extra = Vec::new();
        chunk(&mut extra, b"zzZZ", b"annotation");
        ancillary.splice(33..33, extra);
        assert_eq!(decode_png(&ancillary, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[7, 7, 7, 255]);
    }

    #[test]
    fn png_output_buffer_too_small_is_refused() {
        let _guard = scratch_guard();
        let png = build_png(2, 2, 0, 0, None, None, &[0, 1, 2, 0, 3, 4]);
        let mut small = [0u8; 15];
        assert!(decode_png(&png, &mut small).is_err());
    }

    // -- GIF tests ---------------------------------------------------------

    #[test]
    fn gif_1x1_without_transparency_decodes() {
        let _guard = scratch_guard();
        let gif = build_gif(1, 1, None, 0, 0, 1, 1, &[1], false);
        let mut out = [0u8; 4];
        assert_eq!(decode_gif(&gif, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[255, 0, 0, 255]);
    }

    #[test]
    fn gif_1x1_with_transparency_decodes() {
        let _guard = scratch_guard();
        let gif = build_gif(1, 1, Some(1), 0, 0, 1, 1, &[1], false);
        let mut out = [0u8; 4];
        assert_eq!(decode_gif(&gif, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[255, 0, 0, 0]);

        let gif = build_gif(1, 1, Some(0), 0, 0, 1, 1, &[2], false);
        let mut out = [0u8; 4];
        assert_eq!(decode_gif(&gif, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[0, 255, 0, 255], "index 2 is not the transparent one");
    }

    #[test]
    fn gif_frame_is_placed_at_its_offset_on_the_canvas() {
        let _guard = scratch_guard();
        let gif = build_gif(2, 2, None, 1, 1, 1, 1, &[2], false);
        let mut out = [0u8; 16];
        assert_eq!(decode_gif(&gif, &mut out), Ok((2, 2, 16)));
        assert_eq!(&out[0..12], &[0u8; 12]);
        assert_eq!(&out[12..16], &[0, 255, 0, 255]);
    }

    #[test]
    fn gif_interlaced_rows_are_reordered() {
        let _guard = scratch_guard();
        // A 2x3 frame is transmitted in the order row 0, row 2, row 1.
        let gif = build_gif(2, 3, None, 0, 0, 2, 3, &[1, 1, 2, 2, 3, 3], true);
        let mut out = [0u8; 2 * 3 * 4];
        assert_eq!(decode_gif(&gif, &mut out), Ok((2, 3, 24)));
        assert_eq!(&out[0..8], &[255, 0, 0, 255, 255, 0, 0, 255]);
        assert_eq!(&out[8..16], &[0, 0, 255, 255, 0, 0, 255, 255]);
        assert_eq!(&out[16..24], &[0, 255, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn gif_truncated_input_is_refused() {
        let _guard = scratch_guard();
        let gif = build_gif(1, 1, None, 0, 0, 1, 1, &[1], false);
        let mut out = [0u8; 4];
        assert!(decode_gif(&gif[..34], &mut out).is_err());
        assert!(decode_gif(&gif[..36], &mut out).is_err());
        assert!(decode_gif(&gif[..3], &mut out).is_err());

        let mut bad_magic = gif.clone();
        bad_magic[0] = b'J';
        assert!(decode_gif(&bad_magic, &mut out).is_err());
    }

    #[test]
    fn gif_output_buffer_too_small_is_refused() {
        let _guard = scratch_guard();
        let gif = build_gif(1, 1, None, 0, 0, 1, 1, &[1], false);
        let mut small = [0u8; 3];
        assert!(decode_gif(&gif, &mut small).is_err());
    }

    #[test]
    fn gif_header_reports_the_logical_screen() {
        let gif = build_gif(2, 3, None, 0, 0, 1, 1, &[1], false);
        assert_eq!(decode_gif_header(&gif), Ok((2, 3)));
    }

    // -- dispatch ----------------------------------------------------------

    #[test]
    fn decode_image_routes_png_and_gif() {
        let _guard = scratch_guard();
        let mut out = [0u8; 16];
        assert_eq!(
            decode_image(PNG_FIXED_HUFFMAN_RGB_2X2, b"photo.png", &mut out),
            Ok((2, 2, 16))
        );
        assert_eq!(&out[0..4], &[10, 20, 30, 255]);

        // The signature decides when the name is unhelpful.
        assert_eq!(
            decode_image(PNG_FIXED_HUFFMAN_RGB_2X2, b"mystery", &mut out),
            Ok((2, 2, 16))
        );

        let gif = build_gif(1, 1, None, 0, 0, 1, 1, &[3], false);
        let mut out = [0u8; 4];
        assert_eq!(decode_image(&gif, b"ANIM.GIF", &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[0, 0, 255, 255]);
    }
}

