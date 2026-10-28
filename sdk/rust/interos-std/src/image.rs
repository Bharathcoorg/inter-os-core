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

/// The largest side an `.ico` entry may claim.
///
/// WHY this is far below [`MAX_DECODE_DIMENSION`]: an icon container holds
/// the same mark at a handful of sizes between sixteen and fifty-one-two
/// pixels, and it is the file a page points at for its own picture. Without
/// its own bound, one directory entry could ask the reader for a picture
/// large enough to fill the whole image arena for a decoration.
const MAX_ICO_DIMENSION: u32 = 512;

/// The most entries read out of one `.ico` directory.
///
/// WHY a bound at all: the count is a sixteen-bit number the file chooses,
/// and every entry is sixteen bytes of a buffer the reader has to trust.
/// Twenty is more than a real icon file has ever carried.
const MAX_ICO_ENTRIES: usize = 20;

/// One picture inside an `.ico` container, already checked against the file.
#[derive(Clone, Copy)]
struct IcoEntry {
    width: u32,
    height: u32,
    offset: usize,
    length: usize,
}

/// The width or height an `.ico` directory records: a zero byte means the
/// two-fifty-sixth size, because the field is eight bits and cannot say it.
fn ico_dimension(byte: u8) -> u32 {
    if byte == 0 {
        256
    } else {
        byte as u32
    }
}

/// The size of the picture behind one directory entry, read from the payload
/// rather than from the directory.
///
/// WHY the payload decides: the directory's own dimensions are a summary the
/// writer fills in by hand, and a file whose two accounts disagree is a file
/// that would have the reader size its buffer wrong.
fn ico_payload_size(payload: &[u8]) -> Option<(u32, u32)> {
    if payload.len() >= 8 && payload[..8] == PNG_SIGNATURE {
        let (width, height) = decode_png_header(payload).ok()?;
        return Some((width, height));
    }
    if payload.len() < 40
        || u32::from_le_bytes(payload[0..4].try_into().ok()?) != 40
        || u16::from_le_bytes(payload[12..14].try_into().ok()?) != 1
    {
        return None;
    }
    let width = i32::from_le_bytes(payload[4..8].try_into().ok()?).try_into().ok()?;
    // The header's height counts the pixels and the AND mask under them, so
    // the picture itself is half of it.
    let doubled = i32::from_le_bytes(payload[8..12].try_into().ok()?);
    if doubled <= 0 || doubled % 2 != 0 {
        return None;
    }
    let height = (doubled / 2) as u32;
    let depth = u16::from_le_bytes(payload[14..16].try_into().ok()?);
    let compression = u32::from_le_bytes(payload[16..20].try_into().ok()?);
    if (depth != 24 && depth != 32) || compression != 0 {
        return None;
    }
    Some((width, height))
}

/// The largest picture in an `.ico` container that this reader can rasterise.
///
/// WHY the largest and not the first: a favicon file carries the same mark at
/// sixteen, thirty-two and forty-eight pixels so that the reader can pick the
/// one nearest the size it has, and the entries are not ordered by size.
///
/// WHY an unreadable entry is passed over rather than failed on: a directory
/// that also holds a format this reader was built without -- four-bit colour,
/// a bit-field header -- is an ordinary icon file, and the picture it does
/// carry should still draw.
fn ico_pick(data: &[u8]) -> Result<IcoEntry, ()> {
    if data.len() < 22 || data[..4] != [0, 0, 1, 0] || data[4..6] == [0, 0] {
        return Err(());
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    if count > MAX_ICO_ENTRIES || data.len() < 6 + count * 16 {
        return Err(());
    }
    let mut best: Option<IcoEntry> = None;
    for index in 0..count {
        let directory = 6 + index * 16;
        let length = u32::from_le_bytes(data[directory + 8..directory + 12].try_into().map_err(|_| ())?) as usize;
        let offset = u32::from_le_bytes(data[directory + 12..directory + 16].try_into().map_err(|_| ())?) as usize;
        let Some(end) = offset.checked_add(length) else {
            continue;
        };
        if end > data.len() || length == 0 {
            continue;
        }
        let payload = &data[offset..end];
        let Some((width, height)) = ico_payload_size(payload) else {
            continue;
        };
        if width == 0
            || height == 0
            || width > MAX_ICO_DIMENSION
            || height > MAX_ICO_DIMENSION
            || width > ico_dimension(data[directory])
            || height > ico_dimension(data[directory + 1])
        {
            continue;
        }
        let candidate = IcoEntry {
            width,
            height,
            offset,
            length,
        };
        if best
            .map(|held| width * height > held.width * held.height)
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }
    best.ok_or(())
}

/// Reads the size of the picture an `.ico` container would be drawn from.
pub fn decode_ico_header(data: &[u8]) -> Result<(u32, u32), ()> {
    let entry = ico_pick(data)?;
    Ok((entry.width, entry.height))
}

/// Decodes the largest picture inside an `.ico` container.
///
/// WHY a container needs its own reader: the icon form is a directory of
/// device-independent bitmaps, and the bitmaps drop the file header the
/// `.bmp` reader starts from, double their own height to make room for a
/// one-bit mask underneath, and sometimes leave that mask as the only place
/// transparency was ever written.
pub fn decode_ico(data: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    let entry = ico_pick(data)?;
    let payload = data
        .get(entry.offset..entry.offset + entry.length)
        .ok_or(())?;
    if payload.len() >= 8 && payload[..8] == PNG_SIGNATURE {
        return decode_png(payload, out_rgba);
    }
    decode_ico_dib(payload, entry.width, entry.height, out_rgba)
}

/// Rasterises one device-independent bitmap held inside an `.ico` entry.
fn decode_ico_dib(
    payload: &[u8],
    width: u32,
    height: u32,
    out_rgba: &mut [u8],
) -> Result<(u32, u32, usize), ()> {
    let (width, height) = (width as usize, height as usize);
    let depth = u16::from_le_bytes(payload[14..16].try_into().map_err(|_| ())?) as usize;
    let bytes = depth / 8;
    let stride = (width * bytes + 3) / 4 * 4;
    let pixels = width.checked_mul(height).ok_or(())?;
    let xor_len = height.checked_mul(stride).ok_or(())?;
    if payload.len() < 40 + xor_len || out_rgba.len() < pixels * 4 {
        return Err(());
    }
    // A thirty-two-bit entry can carry opacity in its fourth byte, and many
    // writers leave that byte zero and put the shape in the mask alone. Which
    // of the two this file means is answered by the file: an alpha byte used
    // anywhere is an alpha channel, and none is a mask.
    let uses_alpha = depth == 32 && payload[40..40 + xor_len].chunks_exact(4).any(|p| p[3] != 0);
    let mask_stride = (width + 31) / 32 * 4;
    let mask_start = 40 + xor_len;
    let has_mask = payload.len() >= mask_start + height * mask_stride;
    for row in 0..height {
        // Both arrays are stored bottom-up, as a positive header height says.
        let source = 40 + (height - 1 - row) * stride;
        let mask_row = if has_mask { mask_start + (height - 1 - row) * mask_stride } else { 0 };
        let target = row * width * 4;
        for column in 0..width {
            let pixel = source + column * bytes;
            let erased = has_mask
                && payload[mask_row + column / 8] >> (7 - column % 8) & 1 == 1;
            let alpha = if erased {
                0
            } else if uses_alpha {
                payload[pixel + 3]
            } else {
                255
            };
            let out = target + column * 4;
            out_rgba[out] = payload[pixel + 2];
            out_rgba[out + 1] = payload[pixel + 1];
            out_rgba[out + 2] = payload[pixel];
            out_rgba[out + 3] = alpha;
        }
    }
    Ok((width as u32, height as u32, pixels * 4))
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

#[derive(Clone, Copy)]
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
    /// Whether a transparent index leaves the pixel already on the canvas.
    ///
    /// WHY the two modes: a still decode owns a cleared canvas, so a
    /// transparent pixel must be written as alpha zero for a caller to see
    /// that the frame does not cover it. An animation composites onto the
    /// previous frame, where a transparent pixel means "keep what is under
    /// it" and writing anything would erase the picture.
    skip_transparent: bool,
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
        let transparent = self.transparent == Some(index);
        if !(transparent && self.skip_transparent) {
            let entry = index as usize * 3;
            self.out[offset] = self.palette[entry];
            self.out[offset + 1] = self.palette[entry + 1];
            self.out[offset + 2] = self.palette[entry + 2];
            self.out[offset + 3] = if transparent { 0 } else { 255 };
        }
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
    skip_transparent: bool,
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
        skip_transparent,
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
                    false,
                )?;
                return Ok((header.width, header.height, required));
            }
            _ => return Err(()),
        }
    }
}

// ---------------------------------------------------------------------------
// GIF animation
// ---------------------------------------------------------------------------

/// What a GIF stream declares before any frame is decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GifAnimationInfo {
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    /// 0 means loop forever; 1 means play once.
    ///
    /// WHY the value is reported raw: the Netscape extension stores the count
    /// the encoder asked for, and a decoder that translated it would be
    /// reporting its own policy as the document's property.
    pub loop_count: u16,
}

/// One frame's placement and timing inside a GIF's logical screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GifFrameInfo {
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    /// Delay before the next frame, in hundredths of a second, as stored.
    /// Zero means the document did not state one.
    pub delay_cs: u16,
    /// Graphic Control Extension disposal method, 0..=3.
    pub disposal: u8,
}

/// Walks a GIF's block structure without decoding pixels.
///
/// This is how a caller decides whether a file is an animation before it
/// commits a canvas: the block chain is bounded by the file length and has no
/// LZW work in it.
pub fn gif_animation_info(data: &[u8]) -> Result<GifAnimationInfo, ()> {
    let header = gif_parse_header(data)?;
    let mut pos = header.pos;
    let mut frames = 0u32;
    let mut loop_count = 1u16;
    loop {
        if pos >= data.len() {
            return Err(());
        }
        match data[pos] {
            0x3B => break,
            0x00 => pos += 1,
            0x21 => {
                if pos + 2 > data.len() {
                    return Err(());
                }
                let label = data[pos + 1];
                let block = pos + 2;
                // Netscape looping extension: a 0x0B sub-block holding
                // "NETSCAPE2.0", then a 3-byte sub-block 0x01 <lo> <hi>.
                // Only that one sub-block is interpreted; other application
                // extensions are skipped as opaque data.
                if label == 0xFF
                    && block + 17 <= data.len()
                    && data[block] == 11
                    && &data[block + 1..block + 12] == b"NETSCAPE2.0"
                    && data[block + 12] == 3
                    && data[block + 13] == 1
                {
                    loop_count = u16::from_le_bytes([data[block + 14], data[block + 15]]);
                }
                pos = gif_skip_sub_blocks(data, block)?;
            }
            0x2C => {
                if pos + 10 > data.len() {
                    return Err(());
                }
                let packed = data[pos + 9];
                let mut next = pos + 10;
                if packed & 0x80 != 0 {
                    let entries = 2usize << (packed & 0x07);
                    next = next.checked_add(entries * 3).ok_or(())?;
                }
                if next >= data.len() {
                    return Err(());
                }
                next += 1; // LZW minimum code size
                pos = gif_skip_sub_blocks(data, next)?;
                frames = frames.checked_add(1).ok_or(())?;
            }
            _ => return Err(()),
        }
    }
    if frames == 0 {
        return Err(());
    }
    Ok(GifAnimationInfo {
        width: header.width,
        height: header.height,
        frame_count: frames,
        loop_count,
    })
}

/// Incremental GIF animation decoder.
///
/// The caller owns the canvas and a restore buffer:
///
/// - `canvas` is the logical screen in RGBA, exactly as `decode_gif` lays it
///   out. Clear it before decoding the first frame; a transparent pixel in a
///   frame leaves whatever the canvas already holds.
/// - `restore` is scratch for disposal method 3 (restore to previous). It
///   holds one frame rectangle, contiguously, so it only has to be as large as
///   the largest frame whose disposal is 3. Passing a screen-sized buffer is
///   always sufficient.
///
/// The decoder applies the previous frame's disposal method at the start of
/// the next call, which is the point the specification places it: after the
/// frame has been shown for its delay and before the next frame is drawn.
pub struct GifAnimation<'a> {
    data: &'a [u8],
    header: GifHeader,
    info: GifAnimationInfo,
    pos: usize,
    transparent: Option<u8>,
    disposal: u8,
    delay_cs: u16,
    /// Disposal method and rectangle of the frame currently on the canvas.
    pending: Option<(u8, usize, usize, usize, usize)>,
    frame_index: u32,
    finished: bool,
}

impl<'a> GifAnimation<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, ()> {
        let header = gif_parse_header(data)?;
        let info = gif_animation_info(data)?;
        Ok(Self {
            data,
            header,
            info,
            pos: header.pos,
            transparent: None,
            disposal: 0,
            delay_cs: 0,
            pending: None,
            frame_index: 0,
            finished: false,
        })
    }

    pub fn info(&self) -> GifAnimationInfo {
        self.info
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.header.width, self.header.height)
    }

    pub fn frame_index(&self) -> u32 {
        self.frame_index
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Restarts at the first frame. The caller must clear the canvas first:
    /// a loop starts from the background, not from the last frame on screen.
    pub fn rewind(&mut self) {
        self.pos = self.header.pos;
        self.transparent = None;
        self.disposal = 0;
        self.delay_cs = 0;
        self.pending = None;
        self.frame_index = 0;
        self.finished = false;
    }

    /// Decodes the next frame onto `canvas` and returns its placement and
    /// timing. Returns `Err` at the trailer or on a malformed stream; the two
    /// are distinguished by `is_finished`.
    pub fn decode_next(&mut self, canvas: &mut [u8], restore: &mut [u8]) -> Result<GifFrameInfo, ()> {
        if self.finished {
            return Err(());
        }
        let screen_w = self.header.width as usize;
        let screen_h = self.header.height as usize;
        let required = screen_w
            .checked_mul(screen_h)
            .ok_or(())?
            .checked_mul(4)
            .ok_or(())?;
        if canvas.len() < required {
            return Err(());
        }
        self.apply_pending(canvas, restore)?;

        loop {
            if self.pos >= self.data.len() {
                return Err(());
            }
            match self.data[self.pos] {
                0x3B => {
                    self.finished = true;
                    self.pending = None;
                    return Err(());
                }
                0x00 => self.pos += 1,
                0x21 => {
                    if self.pos + 2 > self.data.len() {
                        return Err(());
                    }
                    let label = self.data[self.pos + 1];
                    self.pos += 2;
                    if label == 0xF9 {
                        // 0x21 0xF9 04 <packed> <delay lo> <delay hi>
                        // <transparent index> 00.
                        if self.pos + 6 > self.data.len()
                            || self.data[self.pos] != 4
                            || self.data[self.pos + 5] != 0
                        {
                            return Err(());
                        }
                        let packed = self.data[self.pos + 1];
                        // Bits 2..=4 are the disposal method. Values 4..=7 are
                        // reserved; treat them as "do not dispose", which is
                        // what viewers do and keeps a malformed extension from
                        // discarding a valid frame.
                        self.disposal = match (packed >> 2) & 0x07 {
                            value @ 0..=3 => value,
                            _ => 0,
                        };
                        self.delay_cs =
                            u16::from_le_bytes([self.data[self.pos + 2], self.data[self.pos + 3]]);
                        self.transparent = if packed & 1 != 0 {
                            Some(self.data[self.pos + 4])
                        } else {
                            None
                        };
                        self.pos += 6;
                    } else {
                        self.pos = gif_skip_sub_blocks(self.data, self.pos)?;
                    }
                }
                0x2C => {
                    let descriptor = self.pos;
                    if descriptor + 10 > self.data.len() {
                        return Err(());
                    }
                    let left =
                        u16::from_le_bytes([self.data[descriptor + 1], self.data[descriptor + 2]])
                            as usize;
                    let top =
                        u16::from_le_bytes([self.data[descriptor + 3], self.data[descriptor + 4]])
                            as usize;
                    let frame_w =
                        u16::from_le_bytes([self.data[descriptor + 5], self.data[descriptor + 6]])
                            as usize;
                    let frame_h =
                        u16::from_le_bytes([self.data[descriptor + 7], self.data[descriptor + 8]])
                            as usize;
                    let packed = self.data[descriptor + 9];
                    self.pos = descriptor + 10;
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
                        if self.pos + entries * 3 > self.data.len() {
                            return Err(());
                        }
                        let offset = self.pos;
                        self.pos += entries * 3;
                        (offset, entries)
                    } else {
                        match self.header.gct {
                            Some((offset, entries)) => (offset, entries),
                            None => return Err(()),
                        }
                    };
                    if self.pos >= self.data.len() {
                        return Err(());
                    }
                    let min_code_size = self.data[self.pos];
                    self.pos += 1;

                    // Disposal 3 restores the rectangle as it looked before
                    // this frame, so the copy has to be taken first.
                    if self.disposal == 3 {
                        self.save_rect(canvas, restore, left, top, frame_w, frame_h)?;
                    }

                    let palette =
                        &self.data[palette_offset..palette_offset + palette_entries * 3];
                    gif_lzw_decode(
                        self.data,
                        self.pos,
                        min_code_size,
                        frame_w,
                        frame_h,
                        packed & 0x40 != 0,
                        screen_w,
                        left,
                        top,
                        &mut canvas[..required],
                        palette,
                        palette_entries,
                        self.transparent,
                        true,
                    )?;
                    // The LZW stream is length-prefixed; skip its chain so a
                    // later call continues at the next block, not inside it.
                    self.pos = gif_skip_sub_blocks(self.data, self.pos)?;

                    self.pending = Some((self.disposal, left, top, frame_w, frame_h));
                    self.frame_index += 1;
                    let info = GifFrameInfo {
                        left: left as u16,
                        top: top as u16,
                        width: frame_w as u16,
                        height: frame_h as u16,
                        delay_cs: self.delay_cs,
                        disposal: self.disposal,
                    };
                    // A Graphic Control Extension governs one frame. A later
                    // frame without one uses the defaults, not this frame's
                    // transparency, disposal or delay.
                    self.transparent = None;
                    self.disposal = 0;
                    self.delay_cs = 0;
                    return Ok(info);
                }
                _ => return Err(()),
            }
        }
    }

    fn save_rect(
        &self,
        canvas: &[u8],
        restore: &mut [u8],
        left: usize,
        top: usize,
        frame_w: usize,
        frame_h: usize,
    ) -> Result<(), ()> {
        let screen_w = self.header.width as usize;
        let row_bytes = frame_w.checked_mul(4).ok_or(())?;
        let needed = row_bytes.checked_mul(frame_h).ok_or(())?;
        if restore.len() < needed {
            return Err(());
        }
        for y in 0..frame_h {
            let start = ((top + y) * screen_w + left) * 4;
            if start + row_bytes > canvas.len() {
                return Err(());
            }
            restore[y * row_bytes..(y + 1) * row_bytes]
                .copy_from_slice(&canvas[start..start + row_bytes]);
        }
        Ok(())
    }

    fn apply_pending(&mut self, canvas: &mut [u8], restore: &mut [u8]) -> Result<(), ()> {
        let Some((disposal, left, top, frame_w, frame_h)) = self.pending.take() else {
            return Ok(());
        };
        let screen_w = self.header.width as usize;
        let row_bytes = frame_w.checked_mul(4).ok_or(())?;
        match disposal {
            0 | 1 => Ok(()),
            // Restore to background. The canvas starts transparent, and the
            // gallery has no opaque media background, so the honest
            // restoration here is the same transparent black the static
            // decoder leaves outside a frame.
            2 => {
                for y in 0..frame_h {
                    let start = ((top + y) * screen_w + left) * 4;
                    if start + row_bytes > canvas.len() {
                        return Err(());
                    }
                    canvas[start..start + row_bytes].fill(0);
                }
                Ok(())
            }
            3 => {
                let needed = row_bytes.checked_mul(frame_h).ok_or(())?;
                if restore.len() < needed {
                    return Err(());
                }
                for y in 0..frame_h {
                    let start = ((top + y) * screen_w + left) * 4;
                    if start + row_bytes > canvas.len() {
                        return Err(());
                    }
                    canvas[start..start + row_bytes]
                        .copy_from_slice(&restore[y * row_bytes..(y + 1) * row_bytes]);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// JPEG (baseline sequential DCT, Huffman only)
// ---------------------------------------------------------------------------

/// The order in which the 64 coefficients of a block are transmitted.
///
/// Position `i` in the scan corresponds to position `ZIGZAG[i]` in the natural
/// row-major block. Quantization tables use the same order.
const JPEG_ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// One basis vector of the 8x8 inverse DCT, already scaled by one half.
///
/// WHY a table: `basis[u][x] = 0.5 * C(u) * cos((2x+1)u*pi/16)`, so the 2-D
/// inverse is simply `sum_u sum_v basis[u][x] * basis[v][y] * F[u][v]`.
/// Hard-coding the cosine terms keeps the decoder free of libm, which a
/// `no_std` crate does not have.
const JPEG_IDCT_BASIS: [[f32; 8]; 8] = [
    [0.35355339, 0.35355339, 0.35355339, 0.35355339, 0.35355339, 0.35355339, 0.35355339, 0.35355339],
    [0.49039264, 0.41573481, 0.27778512, 0.09754516, -0.09754516, -0.27778512, -0.41573481, -0.49039264],
    [0.46193977, 0.19134172, -0.19134172, -0.46193977, -0.46193977, -0.19134172, 0.19134172, 0.46193977],
    [0.41573481, -0.09754516, -0.49039264, -0.27778512, 0.27778512, 0.49039264, 0.09754516, -0.41573481],
    [0.35355339, -0.35355339, -0.35355339, 0.35355339, 0.35355339, -0.35355339, -0.35355339, 0.35355339],
    [0.27778512, -0.49039264, 0.09754516, 0.41573481, -0.41573481, -0.09754516, 0.49039264, -0.27778512],
    [0.19134172, -0.46193977, 0.46193977, -0.19134172, -0.19134172, 0.46193977, -0.46193977, 0.19134172],
    [0.09754516, -0.27778512, 0.41573481, -0.49039264, 0.49039264, -0.41573481, 0.27778512, -0.09754516],
];

/// The largest Huffman symbol count a JPEG table can declare.
const JPEG_HUFF_SYMBOLS: usize = 256;

/// A canonical JPEG Huffman decoding table.
///
/// WHY counts and a sorted symbol list rather than a lookup tree: JPEG gives
/// only code lengths, and the canonical construction sorts symbols by length
/// then value. Decoding walks one bit at a time and compares against the first
/// code of each length, which is exact and needs no table larger than the
/// alphabet itself.
#[derive(Clone, Copy)]
struct JpegHuffmanTable {
    present: bool,
    count: [u8; 17],
    first_code: [u32; 17],
    first_index: [u16; 17],
    symbol: [u8; JPEG_HUFF_SYMBOLS],
}

impl JpegHuffmanTable {
    const EMPTY: Self = Self {
        present: false,
        count: [0; 17],
        first_code: [0; 17],
        first_index: [0; 17],
        symbol: [0; JPEG_HUFF_SYMBOLS],
    };

    fn build(&mut self, counts: &[u8; 16], symbols: &[u8]) -> Result<(), ()> {
        let total: usize = counts.iter().map(|&count| count as usize).sum();
        if total == 0 || total > JPEG_HUFF_SYMBOLS || symbols.len() != total {
            return Err(());
        }
        let mut count = [0u8; 17];
        let mut first_code = [0u32; 17];
        let mut first_index = [0u16; 17];
        let mut symbol = [0u8; JPEG_HUFF_SYMBOLS];
        let mut code = 0u32;
        let mut index = 0usize;
        for length in 1..=16 {
            let n = counts[length - 1] as u32;
            count[length] = n as u8;
            first_code[length] = code;
            first_index[length] = index as u16;
            code = code.checked_add(n).ok_or(())?;
            // More codes of this length than the tree has room for cannot be
            // decoded. An incomplete tree is legal and is left alone.
            if code > (1u32 << length) {
                return Err(());
            }
            code <<= 1;
            index += n as usize;
        }
        symbol[..total].copy_from_slice(symbols);
        self.count = count;
        self.first_code = first_code;
        self.first_index = first_index;
        self.symbol = symbol;
        self.present = true;
        Ok(())
    }

    fn decode(&self, bits: &mut JpegBitReader<'_>) -> Result<u8, ()> {
        if !self.present {
            return Err(());
        }
        let mut code = 0u32;
        for length in 1..=16 {
            code = (code << 1) | bits.bits(1)?;
            let count = self.count[length] as u32;
            if count != 0 {
                let first = self.first_code[length];
                if code >= first && code < first + count {
                    let index = self.first_index[length] as usize + (code - first) as usize;
                    return Ok(self.symbol[index]);
                }
            }
        }
        Err(())
    }
}

/// An MSB-first reader over an entropy-coded JPEG segment.
///
/// The segment is not a plain byte stream: `FF 00` stands for a literal `FF`,
/// while any other `FF xx` is a marker that ends the segment. A marker is left
/// unconsumed so the caller can read restart intervals and the next header.
struct JpegBitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> JpegBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    fn next_entropy_byte(&mut self) -> Option<u8> {
        if self.pos >= self.data.len() {
            return None;
        }
        let byte = self.data[self.pos];
        if byte == 0xFF {
            if self.pos + 1 >= self.data.len() {
                return None;
            }
            if self.data[self.pos + 1] == 0x00 {
                self.pos += 2;
                Some(0xFF)
            } else {
                None
            }
        } else {
            self.pos += 1;
            Some(byte)
        }
    }

    fn bits(&mut self, n: u32) -> Result<u32, ()> {
        if n == 0 {
            return Ok(0);
        }
        while self.count < n {
            let byte = self.next_entropy_byte().ok_or(())?;
            self.buf = (self.buf << 8) | byte as u64;
            self.count += 8;
        }
        let value = ((self.buf >> (self.count - n)) & ((1u64 << n) - 1)) as u32;
        self.count -= n;
        Ok(value)
    }

    /// Discards the bits that pad the current byte at a restart boundary.
    fn align_to_byte(&mut self) {
        self.count -= self.count % 8;
        if self.count == 0 {
            self.buf = 0;
        }
    }

    /// Consumes the `RSTn` that separates two restart intervals.
    ///
    /// The marker is not part of the bit stream, so the partial byte before it
    /// is discarded first. Fill bytes (`FF FF`) before the marker are allowed.
    fn consume_restart(&mut self, expected: u8) -> Result<(), ()> {
        self.align_to_byte();
        while self.pos + 1 < self.data.len()
            && self.data[self.pos] == 0xFF
            && self.data[self.pos + 1] == 0xFF
        {
            self.pos += 1;
        }
        if self.pos + 2 > self.data.len() || self.data[self.pos] != 0xFF {
            return Err(());
        }
        let marker = self.data[self.pos + 1];
        if marker != 0xD0 + (expected & 7) {
            return Err(());
        }
        self.pos += 2;
        self.buf = 0;
        self.count = 0;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct JpegComponent {
    id: u8,
    h: u8,
    v: u8,
    tq: u8,
}

impl JpegComponent {
    const EMPTY: Self = Self {
        id: 0,
        h: 1,
        v: 1,
        tq: 0,
    };
}

struct JpegFrame {
    width: u32,
    height: u32,
    ncomp: usize,
    comp: [JpegComponent; 3],
    hmax: u8,
    vmax: u8,
    restart_interval: u16,
    adobe_transform: Option<u8>,
    /// The input offset just past the SOF segment.
    next_pos: usize,
}

struct JpegScan {
    ncomp: usize,
    /// Frame component indices, in the order the scan transmits them.
    comp: [usize; 3],
    dc_table: [u8; 3],
    ac_table: [u8; 3],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JpegColorSpace {
    Gray,
    YCbCr,
    Rgb,
}

/// The largest number of 8x8 blocks one component can contribute to an MCU.
const JPEG_MAX_MCU_BLOCKS: usize = 16;

/// Fixed scratch owned by the JPEG decoder.
///
/// WHY static and not on the stack: the Huffman tables, four quantization
/// tables and the sample planes of one MCU are several kilobytes, and an app in
/// this system runs on a small stack. The buffers are sized by the JPEG format
/// (four tables, at most 16 blocks per component) rather than by any file.
struct JpegScratch {
    dc: [JpegHuffmanTable; 4],
    ac: [JpegHuffmanTable; 4],
    quant: [[u16; 64]; 4],
    quant_present: [bool; 4],
    samples: [[u8; JPEG_MAX_MCU_BLOCKS * 64]; 3],
}

impl JpegScratch {
    const EMPTY: Self = Self {
        dc: [JpegHuffmanTable::EMPTY; 4],
        ac: [JpegHuffmanTable::EMPTY; 4],
        quant: [[0; 64]; 4],
        quant_present: [false; 4],
        samples: [[0; JPEG_MAX_MCU_BLOCKS * 64]; 3],
    };
}

static mut JPEG_SCRATCH: JpegScratch = JpegScratch::EMPTY;
static JPEG_SCRATCH_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

struct JpegScratchGuard(&'static mut JpegScratch);

impl core::ops::Deref for JpegScratchGuard {
    type Target = JpegScratch;
    fn deref(&self) -> &JpegScratch {
        self.0
    }
}

impl core::ops::DerefMut for JpegScratchGuard {
    fn deref_mut(&mut self) -> &mut JpegScratch {
        self.0
    }
}

impl Drop for JpegScratchGuard {
    fn drop(&mut self) {
        JPEG_SCRATCH_LOCK.store(false, core::sync::atomic::Ordering::Release);
    }
}

/// Borrows the JPEG decoder scratch for the duration of one decode.
///
/// SAFETY: the scratch is protected by `JPEG_SCRATCH_LOCK` spinlock, ensuring
/// that on host multi-threaded test runners, concurrent decode operations are
/// safely serialized, while on bare metal single-threaded guest runs the lock
/// acquires immediately without overhead.
fn jpeg_scratch() -> JpegScratchGuard {
    while JPEG_SCRATCH_LOCK
        .compare_exchange_weak(
            false,
            true,
            core::sync::atomic::Ordering::Acquire,
            core::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {
        core::hint::spin_loop();
    }
    unsafe { JpegScratchGuard(&mut *core::ptr::addr_of_mut!(JPEG_SCRATCH)) }
}

fn jpeg_is_sof(marker: u8) -> bool {
    matches!(
        marker,
        0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
    )
}

/// Parses a frame header segment (SOF0 or SOF1).
fn jpeg_parse_sof(
    seg: &[u8],
    adobe_transform: Option<u8>,
    restart_interval: u16,
    next_pos: usize,
) -> Result<JpegFrame, ()> {
    if seg.len() < 6 {
        return Err(());
    }
    let precision = seg[0];
    if precision != 8 {
        return Err(());
    }
    let height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
    let width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
    let ncomp = seg[5] as usize;
    if width == 0
        || height == 0
        || width > MAX_DECODE_DIMENSION
        || height > MAX_DECODE_DIMENSION
    {
        return Err(());
    }
    // One component is grayscale and three are colour. Four components are
    // CMYK/YCCK, which this decoder does not convert.
    if ncomp != 1 && ncomp != 3 {
        return Err(());
    }
    if seg.len() != 6 + ncomp * 3 {
        return Err(());
    }
    let mut comp = [JpegComponent::EMPTY; 3];
    let mut hmax = 1u8;
    let mut vmax = 1u8;
    for i in 0..ncomp {
        let base = 6 + i * 3;
        let h = seg[base + 1] >> 4;
        let v = seg[base + 1] & 15;
        let tq = seg[base + 2];
        if h == 0 || h > 4 || v == 0 || v > 4 || tq > 3 {
            return Err(());
        }
        comp[i] = JpegComponent {
            id: seg[base],
            h,
            v,
            tq,
        };
        if h > hmax {
            hmax = h;
        }
        if v > vmax {
            vmax = v;
        }
    }
    if ncomp == 3 {
        // The decoder upsamples by replication, which is well defined only
        // when every sampling factor divides the largest one.
        for component in comp[..3].iter() {
            if hmax % component.h != 0 || vmax % component.v != 0 {
                return Err(());
            }
        }
        // Baseline sequential DCT limits an MCU to ten blocks.
        let blocks: usize = comp[..3]
            .iter()
            .map(|component| component.h as usize * component.v as usize)
            .sum();
        if blocks > 10 {
            return Err(());
        }
    } else {
        // A single-component scan has one block per MCU; its frame sampling
        // factors do not describe an interleaved layout.
        hmax = 1;
        vmax = 1;
    }
    Ok(JpegFrame {
        width,
        height,
        ncomp,
        comp,
        hmax,
        vmax,
        restart_interval,
        adobe_transform,
        next_pos,
    })
}

/// Walks the marker stream up to the first frame header.
///
/// `scratch` is `Some` when the caller is decoding pixels, in which case the
/// quantization and Huffman tables that precede SOF are kept; a header-only
/// caller passes `None` and the tables are skipped without interpretation.
fn jpeg_parse_headers(
    data: &[u8],
    mut scratch: Option<&mut JpegScratch>,
) -> Result<JpegFrame, ()> {
    if data.len() < 3 || data[0] != 0xFF || data[1] != 0xD8 || data[2] != 0xFF {
        return Err(());
    }
    let mut pos = 2usize;
    let mut adobe_transform = None;
    let mut restart_interval = 0u16;
    loop {
        if pos >= data.len() || data[pos] != 0xFF {
            return Err(());
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            return Err(());
        }
        let marker = data[pos];
        pos += 1;
        match marker {
            0xD8 => return Err(()), // a second start of image
            0xD9 => return Err(()), // end of image before any frame
            0x01 => continue,       // TEM, no length
            0xD0..=0xD7 => continue, // stray restart before the scan
            _ => {}
        }
        if pos + 2 > data.len() {
            return Err(());
        }
        let length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if length < 2 || pos + length > data.len() {
            return Err(());
        }
        let payload_start = pos + 2;
        let payload_end = pos + length;
        let payload = &data[payload_start..payload_end];
        match marker {
            0xEE => {
                if payload.len() >= 12 && &payload[..5] == b"Adobe" {
                    adobe_transform = Some(payload[11]);
                }
            }
            0xC4 => {
                if let Some(table) = scratch.as_mut() {
                    jpeg_parse_dht(payload, *table)?;
                }
            }
            0xDB => {
                if let Some(table) = scratch.as_mut() {
                    jpeg_parse_dqt(payload, *table)?;
                }
            }
            0xDD => {
                if payload.len() != 2 {
                    return Err(());
                }
                restart_interval = u16::from_be_bytes([payload[0], payload[1]]);
            }
            0xC0 | 0xC1 => {
                return jpeg_parse_sof(payload, adobe_transform, restart_interval, payload_end);
            }
            marker if jpeg_is_sof(marker) => {
                // Progressive, lossless and arithmetic frames are refused.
                return Err(());
            }
            _ => {}
        }
        pos = payload_end;
    }
}

fn jpeg_parse_dqt(seg: &[u8], scratch: &mut JpegScratch) -> Result<(), ()> {
    let mut pos = 0usize;
    while pos < seg.len() {
        let pq_tq = seg[pos];
        pos += 1;
        let precision = pq_tq >> 4;
        let id = (pq_tq & 15) as usize;
        if id >= 4 || precision > 1 {
            return Err(());
        }
        if precision == 0 {
            if pos + 64 > seg.len() {
                return Err(());
            }
            for i in 0..64 {
                let value = seg[pos + i] as u16;
                if value == 0 {
                    return Err(());
                }
                scratch.quant[id][JPEG_ZIGZAG[i]] = value;
            }
            pos += 64;
        } else {
            if pos + 128 > seg.len() {
                return Err(());
            }
            for i in 0..64 {
                let value = u16::from_be_bytes([seg[pos + i * 2], seg[pos + i * 2 + 1]]);
                if value == 0 {
                    return Err(());
                }
                scratch.quant[id][JPEG_ZIGZAG[i]] = value;
            }
            pos += 128;
        }
        scratch.quant_present[id] = true;
    }
    Ok(())
}

fn jpeg_parse_dht(seg: &[u8], scratch: &mut JpegScratch) -> Result<(), ()> {
    let mut pos = 0usize;
    while pos < seg.len() {
        if pos + 17 > seg.len() {
            return Err(());
        }
        let tc_th = seg[pos];
        pos += 1;
        let class = tc_th >> 4;
        let id = (tc_th & 15) as usize;
        if class > 1 || id >= 4 {
            return Err(());
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&seg[pos..pos + 16]);
        pos += 16;
        let total: usize = counts.iter().map(|&count| count as usize).sum();
        if total == 0 || total > JPEG_HUFF_SYMBOLS || pos + total > seg.len() {
            return Err(());
        }
        if class == 0 {
            scratch.dc[id].build(&counts, &seg[pos..pos + total])?;
        } else {
            scratch.ac[id].build(&counts, &seg[pos..pos + total])?;
        }
        pos += total;
    }
    Ok(())
}

fn jpeg_parse_sos(seg: &[u8], frame: &JpegFrame) -> Result<JpegScan, ()> {
    if seg.is_empty() {
        return Err(());
    }
    let ns = seg[0] as usize;
    // This decoder reads one complete interleaved scan, which is what baseline
    // files carry. A non-interleaved colour scan is a different layout.
    if ns != frame.ncomp || ns == 0 || ns > 3 {
        return Err(());
    }
    if seg.len() != 1 + ns * 2 + 3 {
        return Err(());
    }
    let mut scan = JpegScan {
        ncomp: ns,
        comp: [0; 3],
        dc_table: [0; 3],
        ac_table: [0; 3],
    };
    for i in 0..ns {
        let id = seg[1 + i * 2];
        let tables = seg[2 + i * 2];
        let dc = tables >> 4;
        let ac = tables & 15;
        if dc > 3 || ac > 3 {
            return Err(());
        }
        let mut found = None;
        for (index, component) in frame.comp[..frame.ncomp].iter().enumerate() {
            if component.id == id {
                found = Some(index);
                break;
            }
        }
        let index = found.ok_or(())?;
        for previous in 0..i {
            if scan.comp[previous] == index {
                return Err(());
            }
        }
        scan.comp[i] = index;
        scan.dc_table[i] = dc;
        scan.ac_table[i] = ac;
    }
    // Baseline and extended-sequential Huffman scans cover every coefficient
    // in one pass. Any other spectral selection is progressive.
    let tail = 1 + ns * 2;
    if seg[tail] != 0 || seg[tail + 1] != 63 || seg[tail + 2] != 0 {
        return Err(());
    }
    Ok(scan)
}

fn jpeg_receive_extend(bits: &mut JpegBitReader<'_>, size: u32) -> Result<i32, ()> {
    if size == 0 {
        return Ok(0);
    }
    if size > 16 {
        return Err(());
    }
    let value = bits.bits(size)? as i32;
    let threshold = 1i32 << (size - 1);
    if value < threshold {
        Ok(value - (1i32 << size) + 1)
    } else {
        Ok(value)
    }
}

fn jpeg_dequantize(level: i32, quant: u16) -> i32 {
    (level as i64 * quant as i64).clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

fn jpeg_idct(coeff: &[i32; 64], out: &mut [u8; 64]) {
    let mut temp = [0f32; 64];
    for v in 0..8 {
        for x in 0..8 {
            let mut sum = 0f32;
            for u in 0..8 {
                sum += JPEG_IDCT_BASIS[u][x] * coeff[v * 8 + u] as f32;
            }
            temp[v * 8 + x] = sum;
        }
    }
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0f32;
            for v in 0..8 {
                sum += JPEG_IDCT_BASIS[v][y] * temp[v * 8 + x];
            }
            let sample = (sum + 128.5) as i32;
            out[y * 8 + x] = if sample < 0 {
                0
            } else if sample > 255 {
                255
            } else {
                sample as u8
            };
        }
    }
}

fn jpeg_decode_block(
    bits: &mut JpegBitReader<'_>,
    dc: &JpegHuffmanTable,
    ac: &JpegHuffmanTable,
    quant: &[u16; 64],
    predictor: &mut i32,
    out: &mut [u8; 64],
) -> Result<(), ()> {
    let mut coeff = [0i32; 64];

    let category = dc.decode(bits)? as u32;
    let difference = jpeg_receive_extend(bits, category)?;
    *predictor = predictor.checked_add(difference).ok_or(())?;
    coeff[0] = jpeg_dequantize(*predictor, quant[0]);

    let mut k = 1usize;
    while k < 64 {
        let rs = ac.decode(bits)?;
        let run = (rs >> 4) as usize;
        let size = (rs & 15) as u32;
        if size == 0 {
            if run == 15 {
                // ZRL: sixteen zero coefficients.
                k += 16;
                continue;
            }
            break; // EOB
        }
        k += run;
        if k >= 64 {
            return Err(());
        }
        let level = jpeg_receive_extend(bits, size)?;
        coeff[JPEG_ZIGZAG[k]] = jpeg_dequantize(level, quant[JPEG_ZIGZAG[k]]);
        k += 1;
    }

    jpeg_idct(&coeff, out);
    Ok(())
}

fn jpeg_color_space(frame: &JpegFrame) -> Result<JpegColorSpace, ()> {
    if frame.ncomp == 1 {
        return Ok(JpegColorSpace::Gray);
    }
    if frame.ncomp != 3 {
        return Err(());
    }
    if let Some(transform) = frame.adobe_transform {
        return match transform {
            0 => Ok(JpegColorSpace::Rgb),
            1 => Ok(JpegColorSpace::YCbCr),
            _ => Err(()), // YCCK and reserved transforms
        };
    }
    let ids = [frame.comp[0].id, frame.comp[1].id, frame.comp[2].id];
    if ids == [b'R', b'G', b'B'] {
        Ok(JpegColorSpace::Rgb)
    } else {
        Ok(JpegColorSpace::YCbCr)
    }
}

fn jpeg_sample(
    samples: &[u8; JPEG_MAX_MCU_BLOCKS * 64],
    h: usize,
    v: usize,
    dx: usize,
    dy: usize,
    hmax: usize,
    vmax: usize,
) -> u8 {
    // Nearest-neighbour upsampling: every output pixel reads the component
    // sample whose cell contains it. The SOF parser has already checked that
    // the sampling factors divide the largest ones, so this is exact.
    let sx = dx * h / hmax;
    let sy = dy * v / vmax;
    let bx = sx / 8;
    let by = sy / 8;
    samples[(by * h + bx) * 64 + (sy % 8) * 8 + (sx % 8)]
}

fn jpeg_clamp_u8(value: i32) -> u8 {
    if value < 0 {
        0
    } else if value > 255 {
        255
    } else {
        value as u8
    }
}

fn jpeg_decode_scan(
    data: &[u8],
    entropy_start: usize,
    frame: &JpegFrame,
    scan: &JpegScan,
    scratch: &mut JpegScratch,
    out_rgba: &mut [u8],
) -> Result<(), ()> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let required = width
        .checked_mul(height)
        .ok_or(())?
        .checked_mul(4)
        .ok_or(())?;
    if out_rgba.len() < required {
        return Err(());
    }

    let color = jpeg_color_space(frame)?;
    let hmax = frame.hmax as usize;
    let vmax = frame.vmax as usize;
    let mcu_w = 8 * hmax;
    let mcu_h = 8 * vmax;
    let mcu_cols = (width + mcu_w - 1) / mcu_w;
    let mcu_rows = (height + mcu_h - 1) / mcu_h;
    let total_mcus = mcu_cols.checked_mul(mcu_rows).ok_or(())?;

    let mut bits = JpegBitReader::new(&data[entropy_start..]);
    let mut predictors = [0i32; 3];
    let mut next_restart = 0u8;
    let mut mcu = 0usize;

    for my in 0..mcu_rows {
        for mx in 0..mcu_cols {
            if frame.restart_interval != 0
                && mcu != 0
                && mcu % frame.restart_interval as usize == 0
            {
                bits.consume_restart(next_restart)?;
                next_restart = (next_restart + 1) & 7;
                predictors = [0; 3];
            }

            for si in 0..scan.ncomp {
                let ci = scan.comp[si];
                let component = &frame.comp[ci];
                let (block_h, block_v) = if frame.ncomp == 1 {
                    (1usize, 1usize)
                } else {
                    (component.h as usize, component.v as usize)
                };
                let dc_id = scan.dc_table[si] as usize;
                let ac_id = scan.ac_table[si] as usize;
                let q_id = component.tq as usize;
                if !scratch.quant_present[q_id] {
                    return Err(());
                }
                for by in 0..block_v {
                    for bx in 0..block_h {
                        let mut block = [0u8; 64];
                        {
                            let dc = &scratch.dc[dc_id];
                            let ac = &scratch.ac[ac_id];
                            let quant = &scratch.quant[q_id];
                            jpeg_decode_block(
                                &mut bits,
                                dc,
                                ac,
                                quant,
                                &mut predictors[ci],
                                &mut block,
                            )?;
                        }
                        let base = (by * block_h + bx) * 64;
                        scratch.samples[ci][base..base + 64].copy_from_slice(&block);
                    }
                }
            }

            let x0 = mx * mcu_w;
            let y0 = my * mcu_h;
            for dy in 0..mcu_h {
                let y = y0 + dy;
                if y >= height {
                    continue;
                }
                for dx in 0..mcu_w {
                    let x = x0 + dx;
                    if x >= width {
                        continue;
                    }
                    let (r, g, b) = match color {
                        JpegColorSpace::Gray => {
                            let luma = jpeg_sample(&scratch.samples[0], 1, 1, dx, dy, 1, 1);
                            (luma, luma, luma)
                        }
                        JpegColorSpace::YCbCr => {
                            let luma = jpeg_sample(
                                &scratch.samples[0],
                                frame.comp[0].h as usize,
                                frame.comp[0].v as usize,
                                dx,
                                dy,
                                hmax,
                                vmax,
                            ) as i32;
                            let cb = jpeg_sample(
                                &scratch.samples[1],
                                frame.comp[1].h as usize,
                                frame.comp[1].v as usize,
                                dx,
                                dy,
                                hmax,
                                vmax,
                            ) as i32
                                - 128;
                            let cr = jpeg_sample(
                                &scratch.samples[2],
                                frame.comp[2].h as usize,
                                frame.comp[2].v as usize,
                                dx,
                                dy,
                                hmax,
                                vmax,
                            ) as i32
                                - 128;
                            (
                                jpeg_clamp_u8(luma + ((91881 * cr) >> 16)),
                                jpeg_clamp_u8(
                                    luma - ((22554 * cb + 46802 * cr) >> 16),
                                ),
                                jpeg_clamp_u8(luma + ((116130 * cb) >> 16)),
                            )
                        }
                        JpegColorSpace::Rgb => (
                            jpeg_sample(
                                &scratch.samples[0],
                                frame.comp[0].h as usize,
                                frame.comp[0].v as usize,
                                dx,
                                dy,
                                hmax,
                                vmax,
                            ),
                            jpeg_sample(
                                &scratch.samples[1],
                                frame.comp[1].h as usize,
                                frame.comp[1].v as usize,
                                dx,
                                dy,
                                hmax,
                                vmax,
                            ),
                            jpeg_sample(
                                &scratch.samples[2],
                                frame.comp[2].h as usize,
                                frame.comp[2].v as usize,
                                dx,
                                dy,
                                hmax,
                                vmax,
                            ),
                        ),
                    };
                    let offset = (y * width + x) * 4;
                    out_rgba[offset] = r;
                    out_rgba[offset + 1] = g;
                    out_rgba[offset + 2] = b;
                    out_rgba[offset + 3] = 255;
                }
            }
            mcu += 1;
        }
    }

    // Returning after fewer MCUs would hand the caller a partially painted
    // picture, which is the same lie as inventing the missing pixels.
    if mcu != total_mcus {
        return Err(());
    }
    Ok(())
}

/// Returns the dimensions a JPEG's frame header declares, without decoding pixels.
///
/// Only a frame this decoder can actually decode is reported: progressive,
/// lossless, arithmetic, 12-bit and CMYK headers are refused here as well.
pub fn decode_jpeg_header(data: &[u8]) -> Result<(u32, u32), ()> {
    let frame = jpeg_parse_headers(data, None)?;
    Ok((frame.width, frame.height))
}

/// Decodes a baseline (SOF0) or extended-sequential (SOF1) Huffman JPEG into
/// an RGBA (4 bytes per pixel) destination buffer.
///
/// Grayscale and three-component YCbCr/RGB images are supported, including
/// 4:4:4, 4:2:2 and 4:2:0 sampling and restart intervals. Progressive,
/// lossless, arithmetic, 12-bit and four-component images are refused.
/// Returns the number of RGBA bytes written to `out_rgba`.
pub fn decode_jpeg(data: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    let mut scratch = jpeg_scratch();
    *scratch = JpegScratch::EMPTY;

    let mut frame = jpeg_parse_headers(data, Some(&mut *scratch))?;
    let mut pos = frame.next_pos;
    loop {
        if pos >= data.len() || data[pos] != 0xFF {
            return Err(());
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            return Err(());
        }
        let marker = data[pos];
        pos += 1;
        match marker {
            0xD8 => return Err(()),
            0xD9 => return Err(()), // end of image before the scan
            0x01 => continue,
            0xD0..=0xD7 => continue,
            _ => {}
        }
        if pos + 2 > data.len() {
            return Err(());
        }
        let length = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if length < 2 || pos + length > data.len() {
            return Err(());
        }
        let payload_start = pos + 2;
        let payload_end = pos + length;
        let payload = &data[payload_start..payload_end];
        match marker {
            0xC4 => jpeg_parse_dht(payload, &mut *scratch)?,
            0xDB => jpeg_parse_dqt(payload, &mut *scratch)?,
            0xDD => {
                if payload.len() != 2 {
                    return Err(());
                }
                frame.restart_interval = u16::from_be_bytes([payload[0], payload[1]]);
            }
            0xEE => {
                if payload.len() >= 12 && &payload[..5] == b"Adobe" {
                    frame.adobe_transform = Some(payload[11]);
                }
            }
            0xDA => {
                let scan = jpeg_parse_sos(payload, &frame)?;
                jpeg_decode_scan(data, payload_end, &frame, &scan, &mut *scratch, out_rgba)?;
                let bytes = (frame.width as usize)
                    .checked_mul(frame.height as usize)
                    .ok_or(())?
                    .checked_mul(4)
                    .ok_or(())?;
                return Ok((frame.width, frame.height, bytes));
            }
            0xC0 | 0xC1 => return Err(()), // a second frame header
            marker if jpeg_is_sof(marker) => return Err(()),
            _ => {}
        }
        pos = payload_end;
    }
}

// ---------------------------------------------------------------------------
// WebP (lossless, VP8L)
// ---------------------------------------------------------------------------

/// The most prefix-code groups a stream decoded here may declare.
///
/// WHY a bound exists: each group carries five complete prefix codes and an
/// entropy image may name thousands of them. This crate runs in a no-heap
/// process with fixed scratch, so a stream that asks for more groups than the
/// scratch holds is refused rather than decoded with an allocation.
const WEBP_MAX_GROUPS: usize = 64;

/// The total number of prefix-code symbols all groups may hold at once.
const WEBP_MAX_SYMBOLS: usize = 1 << 15;

/// The largest transform image: predictor modes or colour coefficients.
const WEBP_MAX_TRANSFORM_PIXELS: usize = 1 << 16;

/// The largest entropy image.
const WEBP_MAX_ENTROPY_PIXELS: usize = 1 << 16;

/// The largest colour cache WebP allows.
const WEBP_MAX_CACHE_BITS: u32 = 11;

/// The green prefix code also codes the 24 length prefixes and the cache.
const WEBP_LITERAL_CODES: usize = 256;
const WEBP_LENGTH_CODES: usize = 24;
const WEBP_GREEN_CODES: usize = WEBP_LITERAL_CODES + WEBP_LENGTH_CODES;
const WEBP_GREEN_ALPHABET: usize = WEBP_GREEN_CODES + (1 << WEBP_MAX_CACHE_BITS);

/// The distance prefix code alphabet.
const WEBP_DISTANCE_CODES: usize = 40;

/// The longest code a WebP prefix code may use.
const WEBP_MAX_CODE_LENGTH: usize = 15;

/// The value `WebpHuffman::single` holds when a tree names more than one symbol.
const WEBP_NO_SYMBOL: u16 = u16::MAX;

/// The order in which the code-length code's own lengths are transmitted.
const WEBP_CODE_LENGTH_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// The close neighbourhood a distance code below 121 names, as (left, up).
const WEBP_DISTANCE_MAP: [(i8, i8); 120] = [
    (0, 1), (1, 0), (1, 1), (-1, 1), (0, 2), (2, 0), (1, 2), (-1, 2), (2, 1), (-2, 1),
    (2, 2), (-2, 2), (0, 3), (3, 0), (1, 3), (-1, 3), (3, 1), (-3, 1), (2, 3), (-2, 3),
    (3, 2), (-3, 2), (0, 4), (4, 0), (1, 4), (-1, 4), (4, 1), (-4, 1), (3, 3), (-3, 3),
    (2, 4), (-2, 4), (4, 2), (-4, 2), (0, 5), (3, 4), (-3, 4), (4, 3), (-4, 3), (5, 0),
    (1, 5), (-1, 5), (5, 1), (-5, 1), (2, 5), (-2, 5), (5, 2), (-5, 2), (4, 4), (-4, 4),
    (3, 5), (-3, 5), (5, 3), (-5, 3), (0, 6), (6, 0), (1, 6), (-1, 6), (6, 1), (-6, 1),
    (2, 6), (-2, 6), (6, 2), (-6, 2), (4, 5), (-4, 5), (5, 4), (-5, 4), (3, 6), (-3, 6),
    (6, 3), (-6, 3), (0, 7), (7, 0), (1, 7), (-1, 7), (5, 5), (-5, 5), (7, 1), (-7, 1),
    (4, 6), (-4, 6), (6, 4), (-6, 4), (2, 7), (-2, 7), (7, 2), (-7, 2), (3, 7), (-3, 7),
    (7, 3), (-7, 3), (5, 6), (-5, 6), (6, 5), (-6, 5), (8, 0), (4, 7), (-4, 7), (7, 4),
    (-7, 4), (8, 1), (8, 2), (6, 6), (-6, 6), (8, 3), (5, 7), (-5, 7), (7, 5), (-7, 5),
    (8, 4), (6, 7), (-6, 7), (7, 6), (-7, 6), (8, 5), (7, 7), (-7, 7), (8, 6), (8, 7),
];

/// A reader over a VP8L bitstream, least-significant bit first.
struct WebpBits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> WebpBits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    fn read(&mut self, n: u32) -> Result<u32, ()> {
        if n == 0 {
            return Ok(0);
        }
        if n > 24 {
            return Err(());
        }
        while self.count < n {
            let byte = *self.data.get(self.pos).ok_or(())?;
            self.buf |= (byte as u64) << self.count;
            self.pos += 1;
            self.count += 8;
        }
        let value = (self.buf & ((1u64 << n) - 1)) as u32;
        self.buf >>= n;
        self.count -= n;
        Ok(value)
    }
}

/// A canonical WebP prefix code.
///
/// WHY counts and a sorted symbol list: WebP transmits only code lengths. The
/// canonical construction sorts symbols by length then value, so decoding walks
/// one bit at a time and compares against the first code of each length. A
/// single-symbol tree consumes no bits at all, which the format requires.
#[derive(Clone, Copy)]
struct WebpHuffman {
    count: [u16; 16],
    first_code: [u16; 16],
    first_index: [u16; 16],
    single: u16,
}

impl WebpHuffman {
    const EMPTY: Self = Self {
        count: [0; 16],
        first_code: [0; 16],
        first_index: [0; 16],
        single: WEBP_NO_SYMBOL,
    };

    fn decode(&self, bits: &mut WebpBits<'_>, symbols: &[u16]) -> Result<usize, ()> {
        if self.single != WEBP_NO_SYMBOL {
            return Ok(self.single as usize);
        }
        let mut code = 0u32;
        for length in 1..=WEBP_MAX_CODE_LENGTH {
            code = (code << 1) | bits.read(1)?;
            let count = self.count[length] as u32;
            if count != 0 {
                let first = self.first_code[length] as u32;
                if code >= first && code - first < count {
                    let index = self.first_index[length] as usize + (code - first) as usize;
                    return symbols.get(index).map(|&symbol| symbol as usize).ok_or(());
                }
            }
        }
        Err(())
    }
}

/// Builds a canonical prefix code from `lengths`, appending its symbols to the
/// shared arena at `used`.
fn webp_build_huffman(
    lengths: &[u8],
    symbols: &mut [u16],
    used: &mut usize,
) -> Result<WebpHuffman, ()> {
    let mut count = [0u16; 16];
    let mut total = 0usize;
    for &length in lengths {
        let length = length as usize;
        if length > WEBP_MAX_CODE_LENGTH {
            return Err(());
        }
        if length != 0 {
            count[length] += 1;
            total += 1;
        }
    }
    if total == 0 {
        return Err(());
    }
    let start = *used;
    if start.checked_add(total).ok_or(())? > symbols.len() {
        return Err(());
    }

    let mut table = WebpHuffman::EMPTY;
    table.count = count;
    let mut code = 0u32;
    let mut index = start;
    for length in 1..=WEBP_MAX_CODE_LENGTH {
        table.first_code[length] = code as u16;
        table.first_index[length] = index as u16;
        code = code.checked_add(count[length] as u32).ok_or(())?;
        // More codes of this length than the tree has room for cannot be
        // decoded. An incomplete tree is legal and is left alone.
        if code > (1u32 << length) {
            return Err(());
        }
        code <<= 1;
        index += count[length] as usize;
    }

    let mut write = start;
    for length in 1..=WEBP_MAX_CODE_LENGTH {
        for (symbol, &symbol_length) in lengths.iter().enumerate() {
            if symbol_length as usize == length {
                symbols[write] = symbol as u16;
                write += 1;
            }
        }
    }
    *used = write;
    if total == 1 {
        table.single = symbols[start];
    }
    Ok(table)
}

/// Reads one prefix code: either the simple form naming one or two symbols, or
/// the normal form whose code lengths are themselves prefix-coded.
fn webp_read_huffman(
    bits: &mut WebpBits<'_>,
    lengths: &mut [u8],
    symbols: &mut [u16],
    used: &mut usize,
) -> Result<WebpHuffman, ()> {
    let alphabet = lengths.len();
    for length in lengths.iter_mut() {
        *length = 0;
    }
    if bits.read(1)? == 1 {
        let count = bits.read(1)? + 1;
        let first_is_byte = bits.read(1)? == 1;
        let first = bits.read(if first_is_byte { 8 } else { 1 })? as usize;
        if first >= alphabet {
            return Err(());
        }
        lengths[first] = 1;
        if count == 2 {
            let second = bits.read(8)? as usize;
            if second >= alphabet {
                return Err(());
            }
            lengths[second] = 1;
        }
        return webp_build_huffman(&lengths[..alphabet], symbols, used);
    }

    let mut code_lengths = [0u8; 19];
    let num_codes = bits.read(4)? as usize + 4;
    for &slot in WEBP_CODE_LENGTH_ORDER.iter().take(num_codes) {
        code_lengths[slot] = bits.read(3)? as u8;
    }
    let code_length_tree = webp_build_huffman(&code_lengths, symbols, used)?;

    let mut max_symbol = alphabet;
    if bits.read(1)? == 1 {
        let nbits = 2 + 2 * bits.read(3)?;
        let value = 2 + bits.read(nbits)? as usize;
        if value > alphabet {
            return Err(());
        }
        max_symbol = value;
    }
    let mut symbol = 0usize;
    // Code 16 repeats the previous non-zero length, not the entry just
    // written: a stream may emit zeros and then ask for that earlier length
    // again. Before any non-zero length has been emitted the seed is 8.
    let mut previous_length = 8u8;
    while symbol < alphabet && max_symbol > 0 {
        max_symbol -= 1;
        let code = code_length_tree.decode(bits, symbols)?;
        if code < 16 {
            lengths[symbol] = code as u8;
            if code != 0 {
                previous_length = code as u8;
            }
            symbol += 1;
            continue;
        }
        let (extra_bits, base) = match code {
            16 => (2u32, 3usize),
            17 => (3, 3),
            18 => (7, 11),
            _ => return Err(()),
        };
        let repeat = bits.read(extra_bits)? as usize + base;
        if symbol.checked_add(repeat).ok_or(())? > alphabet {
            return Err(());
        }
        let value = if code == 16 { previous_length } else { 0 };
        for _ in 0..repeat {
            lengths[symbol] = value;
            symbol += 1;
        }
    }
    webp_build_huffman(&lengths[..alphabet], symbols, used)
}

/// One of the four transforms a VP8L stream may carry.
#[derive(Clone, Copy)]
struct WebpTransform {
    kind: u8,
    bits: u8,
    colors: u16,
    xsize: u32,
    ysize: u32,
    data_w: u32,
}

const WEBP_NO_TRANSFORM: WebpTransform = WebpTransform {
    kind: 0,
    bits: 0,
    colors: 0,
    xsize: 0,
    ysize: 0,
    data_w: 0,
};

/// Fixed scratch shared by the WebP decoding.
///
/// WHY static and not on the stack: the transform images, the prefix codes and
/// the colour cache are too large for an app's stack, and the crate has no
/// heap. The buffers are sized by the constants above rather than by anything
/// a file says, so a stream that names more groups, symbols or blocks than
/// they hold is refused before it can write outside them.
struct WebpScratch {
    groups: [[WebpHuffman; 5]; WEBP_MAX_GROUPS],
    symbols: [u16; WEBP_MAX_SYMBOLS],
    predictor: [u8; WEBP_MAX_TRANSFORM_PIXELS],
    cross_color: [u8; WEBP_MAX_TRANSFORM_PIXELS * 3],
    entropy: [u8; WEBP_MAX_ENTROPY_PIXELS],
    palette: [u32; 256],
    color_cache: [u32; 1 << WEBP_MAX_CACHE_BITS],
    code_lengths: [u8; WEBP_GREEN_ALPHABET],
}

const WEBP_SCRATCH_EMPTY: WebpScratch = WebpScratch {
    groups: [[WebpHuffman::EMPTY; 5]; WEBP_MAX_GROUPS],
    symbols: [0; WEBP_MAX_SYMBOLS],
    predictor: [0; WEBP_MAX_TRANSFORM_PIXELS],
    cross_color: [0; WEBP_MAX_TRANSFORM_PIXELS * 3],
    entropy: [0; WEBP_MAX_ENTROPY_PIXELS],
    palette: [0; 256],
    color_cache: [0; 1 << WEBP_MAX_CACHE_BITS],
    code_lengths: [0; WEBP_GREEN_ALPHABET],
};

static mut WEBP_SCRATCH: WebpScratch = WEBP_SCRATCH_EMPTY;

/// Borrows the WebP decoder scratch for the duration of one decode.
///
/// SAFETY: the scratch is sound because the callers are single-threaded and
/// non-reentrant. Every app in this system runs one event loop on one task and
/// calls `decode_image` from that loop, never from an interrupt handler or a
/// second thread. The VP8L decoder recurses into itself for the sub-resolution
/// images, but it does so by reborrowing the one reference it was handed, never
/// by taking a second one from the static, and the nested decode finishes
/// before its caller resumes. The unit tests serialise themselves on the same
/// mutex the other decoders use.
fn webp_scratch() -> &'static mut WebpScratch {
    unsafe { &mut *core::ptr::addr_of_mut!(WEBP_SCRATCH) }
}

fn webp_div_round_up(value: usize, bits: u32) -> usize {
    (value + (1usize << bits) - 1) >> bits
}

fn webp_color_index_bits(colors: usize) -> u32 {
    if colors > 16 {
        0
    } else if colors > 4 {
        1
    } else if colors > 2 {
        2
    } else {
        3
    }
}

fn webp_pixel(pixels: &[u8], index: usize) -> Result<u32, ()> {
    let offset = index.checked_mul(4).ok_or(())?;
    let end = offset.checked_add(4).ok_or(())?;
    let bytes = pixels.get(offset..end).ok_or(())?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn webp_put_pixel(pixels: &mut [u8], index: usize, value: u32) -> Result<(), ()> {
    let offset = index.checked_mul(4).ok_or(())?;
    let end = offset.checked_add(4).ok_or(())?;
    let bytes = pixels.get_mut(offset..end).ok_or(())?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn webp_add_pixels(a: u32, b: u32) -> u32 {
    let mut out = 0u32;
    for shift in [0u32, 8, 16, 24] {
        let sum = ((a >> shift) & 0xff) + ((b >> shift) & 0xff);
        out |= (sum & 0xff) << shift;
    }
    out
}

fn webp_average2(a: u32, b: u32) -> u32 {
    let mut out = 0u32;
    for shift in [0u32, 8, 16, 24] {
        let average = (((a >> shift) & 0xff) + ((b >> shift) & 0xff)) >> 1;
        out |= average << shift;
    }
    out
}

fn webp_select(left: u32, top: u32, top_left: u32) -> u32 {
    let mut left_error = 0i32;
    let mut top_error = 0i32;
    for shift in [0u32, 8, 16, 24] {
        let l = ((left >> shift) & 0xff) as i32;
        let t = ((top >> shift) & 0xff) as i32;
        let tl = ((top_left >> shift) & 0xff) as i32;
        let estimate = l + t - tl;
        left_error += (estimate - l).abs();
        top_error += (estimate - t).abs();
    }
    if left_error < top_error {
        left
    } else {
        top
    }
}

fn webp_clamped_add_subtract_full(a: u32, b: u32, c: u32) -> u32 {
    let mut out = 0u32;
    for shift in [0u32, 8, 16, 24] {
        let value = ((a >> shift) & 0xff) as i32 + ((b >> shift) & 0xff) as i32
            - ((c >> shift) & 0xff) as i32;
        out |= (value.clamp(0, 255) as u32) << shift;
    }
    out
}

fn webp_clamped_add_subtract_half(a: u32, b: u32) -> u32 {
    let mut out = 0u32;
    for shift in [0u32, 8, 16, 24] {
        let value = ((a >> shift) & 0xff) as i32;
        let other = ((b >> shift) & 0xff) as i32;
        out |= ((value + (value - other) / 2).clamp(0, 255) as u32) << shift;
    }
    out
}

/// The prediction for a pixel in the selected mode, channels independently.
fn webp_predict(mode: u32, left: u32, top: u32, top_left: u32, top_right: u32) -> u32 {
    match mode {
        1 => left,
        2 => top,
        3 => top_right,
        4 => top_left,
        5 => webp_average2(webp_average2(left, top_right), top),
        6 => webp_average2(left, top_left),
        7 => webp_average2(left, top),
        8 => webp_average2(top_left, top),
        9 => webp_average2(top, top_right),
        10 => webp_average2(webp_average2(left, top_left), webp_average2(top, top_right)),
        11 => webp_select(left, top, top_left),
        12 => webp_clamped_add_subtract_full(left, top, top_left),
        13 => webp_clamped_add_subtract_half(webp_average2(left, top), top_left),
        _ => 0xff00_0000,
    }
}

fn webp_color_delta(t: u8, c: u8) -> i32 {
    ((t as i8 as i32) * (c as i8 as i32)) >> 5
}

fn webp_cache_index(pixel: u32, cache_bits: u32) -> usize {
    (pixel.wrapping_mul(0x1e35_a7bd) >> (32 - cache_bits)) as usize
}

fn webp_prefix_value(prefix: u32, bits: &mut WebpBits<'_>) -> Result<u32, ()> {
    if prefix < 4 {
        return Ok(prefix + 1);
    }
    let extra_bits = (prefix - 2) >> 1;
    if extra_bits > 18 {
        return Err(());
    }
    let offset = (2 + (prefix & 1)).checked_shl(extra_bits).ok_or(())?;
    Ok(offset + bits.read(extra_bits)? + 1)
}

/// Turns a distance code into a distance in pixels, clipped as WebP requires.
fn webp_distance(code: usize, width: usize) -> usize {
    if code > WEBP_DISTANCE_MAP.len() {
        return code - WEBP_DISTANCE_MAP.len();
    }
    let pair = match WEBP_DISTANCE_MAP.get(code.wrapping_sub(1)) {
        Some(pair) => *pair,
        None => return 0,
    };
    let distance = pair.0 as i32 + pair.1 as i32 * width as i32;
    if distance < 1 {
        1
    } else {
        distance as usize
    }
}

/// Decodes the entropy-coded pixels of one VP8L image stream into `dst`.
#[allow(clippy::too_many_arguments)]
fn webp_decode_data(
    bits: &mut WebpBits<'_>,
    width: usize,
    height: usize,
    cache_bits: u32,
    prefix_bits: u32,
    groups: &[[WebpHuffman; 5]],
    symbols: &[u16],
    entropy: &[u8],
    entropy_width: usize,
    cache: &mut [u32],
    dst: &mut [u8],
) -> Result<(), ()> {
    let count = width.checked_mul(height).ok_or(())?;
    if width == 0 || dst.len() < count.checked_mul(4).ok_or(())? {
        return Err(());
    }
    let cache_size = if cache_bits == 0 { 0 } else { 1usize << cache_bits };
    if cache.len() < cache_size {
        return Err(());
    }
    for slot in cache[..cache_size].iter_mut() {
        *slot = 0;
    }

    let mut pos = 0usize;
    while pos < count {
        let group_index = if prefix_bits == 0 {
            0
        } else {
            let index = ((pos / width) >> prefix_bits) * entropy_width
                + ((pos % width) >> prefix_bits);
            match entropy.get(index) {
                Some(&group) => group as usize,
                None => {
                    return Err(());
                }
            }
        };
        let group = match groups.get(group_index) {
            Some(group) => group,
            None => {
                return Err(());
            }
        };
        let code = match group[0].decode(bits, symbols) {
            Ok(code) => code,
            Err(()) => {
                return Err(());
            }
        };
        if code < WEBP_LITERAL_CODES {
            let red = match group[1].decode(bits, symbols) {
                Ok(red) => red,
                Err(()) => {
                    return Err(());
                }
            };
            let blue = match group[2].decode(bits, symbols) {
                Ok(blue) => blue,
                Err(()) => {
                    return Err(());
                }
            };
            let alpha = match group[3].decode(bits, symbols) {
                Ok(alpha) => alpha,
                Err(()) => {
                    return Err(());
                }
            };
            let pixel = ((alpha as u32) << 24)
                | ((red as u32) << 16)
                | ((code as u32) << 8)
                | blue as u32;
            if webp_put_pixel(dst, pos, pixel).is_err() {
                return Err(());
            }
            if cache_size != 0 {
                cache[webp_cache_index(pixel, cache_bits)] = pixel;
            }
            pos += 1;
        } else if code < WEBP_GREEN_CODES {
            let length = match webp_prefix_value((code - WEBP_LITERAL_CODES) as u32, bits) {
                Ok(length) => length as usize,
                Err(()) => {
                    return Err(());
                }
            };
            let distance_symbol = match group[4].decode(bits, symbols) {
                Ok(symbol) => symbol,
                Err(()) => {
                    return Err(());
                }
            };
            let distance_code = match webp_prefix_value(distance_symbol as u32, bits) {
                Ok(code) => code as usize,
                Err(()) => {
                    return Err(());
                }
            };
            let distance = webp_distance(distance_code, width);
            if distance == 0 || distance > pos {
                return Err(());
            }
            let end = pos.checked_add(length).ok_or(())?;
            if end > count {
                return Err(());
            }
            let source = pos - distance;
            for offset in 0..length {
                let pixel = match webp_pixel(dst, source + offset) {
                    Ok(pixel) => pixel,
                    Err(()) => {
                        return Err(());
                    }
                };
                if webp_put_pixel(dst, pos + offset, pixel).is_err() {
                    return Err(());
                }
                if cache_size != 0 {
                    cache[webp_cache_index(pixel, cache_bits)] = pixel;
                }
            }
            pos = end;
        } else {
            let index = code - WEBP_GREEN_CODES;
            if index >= cache_size {
                return Err(());
            }
            let pixel = match cache.get(index) {
                Some(&pixel) => pixel,
                None => {
                    return Err(());
                }
            };
            if webp_put_pixel(dst, pos, pixel).is_err() {
                return Err(());
            }
            pos += 1;
        }
    }
    Ok(())
}

fn webp_inverse_color_index(
    dst: &mut [u8],
    source_width: usize,
    height: usize,
    width: usize,
    bits: u32,
    palette: &[u32],
) -> Result<(), ()> {
    let per_pixel = 1usize << bits;
    let bits_per_pixel = 8u32 >> bits;
    let mask = (1u32 << bits_per_pixel) - 1;
    let mut y = height;
    while y > 0 {
        y -= 1;
        let source_row = y.checked_mul(source_width).ok_or(())?;
        let target_row = y.checked_mul(width).ok_or(())?;
        let mut x = width;
        while x > 0 {
            x -= 1;
            let packed = webp_pixel(dst, source_row + x / per_pixel)?;
            let green = (packed >> 8) & 0xff;
            let index = if bits == 0 {
                green
            } else {
                (green >> ((x & (per_pixel - 1)) as u32 * bits_per_pixel)) & mask
            };
            let color = palette.get(index as usize).copied().unwrap_or(0);
            webp_put_pixel(dst, target_row + x, color)?;
        }
    }
    Ok(())
}

fn webp_inverse_predictor(
    dst: &mut [u8],
    width: usize,
    height: usize,
    bits: u32,
    data_width: usize,
    modes: &[u8],
) -> Result<(), ()> {
    for y in 0..height {
        let row = y.checked_mul(width).ok_or(())?;
        for x in 0..width {
            let index = row + x;
            let residual = webp_pixel(dst, index)?;
            let predicted = if y == 0 {
                if x == 0 {
                    0xff00_0000
                } else {
                    webp_pixel(dst, index - 1)?
                }
            } else if x == 0 {
                webp_pixel(dst, index - width)?
            } else {
                let left = webp_pixel(dst, index - 1)?;
                let top = webp_pixel(dst, index - width)?;
                let top_left = webp_pixel(dst, index - width - 1)?;
                let slot = (y >> bits) * data_width + (x >> bits);
                let mode = (*modes.get(slot).ok_or(())? & 0x0f) as u32;
                // The top-right pixel of the rightmost column is the leftmost
                // pixel of the same row, per the format's border rule.
                let top_right = if x + 1 < width {
                    webp_pixel(dst, index - width + 1)?
                } else {
                    webp_pixel(dst, row)?
                };
                webp_predict(mode, left, top, top_left, top_right)
            };
            webp_put_pixel(dst, index, webp_add_pixels(residual, predicted))?;
        }
    }
    Ok(())
}

fn webp_inverse_cross_color(
    dst: &mut [u8],
    width: usize,
    height: usize,
    bits: u32,
    data_width: usize,
    elements: &[u8],
) -> Result<(), ()> {
    for y in 0..height {
        let row = y.checked_mul(width).ok_or(())?;
        for x in 0..width {
            let index = row + x;
            let pixel = webp_pixel(dst, index)?;
            let slot = (y >> bits) * data_width + (x >> bits);
            let offset = slot.checked_mul(3).ok_or(())?;
            let green_to_red = *elements.get(offset).ok_or(())?;
            let green_to_blue = *elements.get(offset + 1).ok_or(())?;
            let red_to_blue = *elements.get(offset + 2).ok_or(())?;
            let green = (pixel >> 8) & 0xff;
            let red = (((pixel >> 16) & 0xff) as i32 + webp_color_delta(green_to_red, green as u8))
                & 0xff;
            let blue = ((pixel & 0xff) as i32 + webp_color_delta(green_to_blue, green as u8)) & 0xff;
            let blue = (blue + webp_color_delta(red_to_blue, red as u8)) & 0xff;
            let value = (pixel & 0xff00_ff00) | ((red as u32) << 16) | blue as u32;
            webp_put_pixel(dst, index, value)?;
        }
    }
    Ok(())
}

fn webp_inverse_add_green(dst: &mut [u8], count: usize) -> Result<(), ()> {
    for index in 0..count {
        let pixel = webp_pixel(dst, index)?;
        let green = (pixel >> 8) & 0xff;
        let red = (((pixel >> 16) & 0xff) + green) & 0xff;
        let blue = ((pixel & 0xff) + green) & 0xff;
        webp_put_pixel(dst, index, (pixel & 0xff00_ff00) | (red << 16) | blue)?;
    }
    Ok(())
}

/// Decodes one VP8L image stream: transforms (main image only), colour cache,
/// prefix codes and pixels, then the inverse transforms.
fn webp_decode_argb(
    bits: &mut WebpBits<'_>,
    width: usize,
    height: usize,
    level0: bool,
    scratch: &mut WebpScratch,
    dst: &mut [u8],
) -> Result<(), ()> {
    let total = width.checked_mul(height).ok_or(())?;
    if width == 0 || height == 0 || dst.len() < total.checked_mul(4).ok_or(())? {
        return Err(());
    }

    let mut transforms = [WEBP_NO_TRANSFORM; 4];
    let mut transform_count = 0usize;
    let mut image_width = width;

    if level0 {
        let mut seen = 0u8;
        loop {
            if bits.read(1)? == 0 {
                break;
            }
            if transform_count == transforms.len() {
                return Err(());
            }
            let kind = bits.read(2)? as u8;
            if seen & (1 << kind) != 0 {
                return Err(());
            }
            seen |= 1 << kind;
            let mut transform = WebpTransform {
                kind,
                bits: 0,
                colors: 0,
                xsize: image_width as u32,
                ysize: height as u32,
                data_w: 0,
            };
            match kind {
                0 | 1 => {
                    let block_bits = bits.read(3)? + 2;
                    let data_w = webp_div_round_up(image_width, block_bits);
                    let data_h = webp_div_round_up(height, block_bits);
                    let area = data_w.checked_mul(data_h).ok_or(())?;
                    if area > WEBP_MAX_TRANSFORM_PIXELS {
                        return Err(());
                    }
                    let bytes = area.checked_mul(4).ok_or(())?;
                    webp_decode_argb(bits, data_w, data_h, false, scratch, &mut dst[..bytes])?;
                    if kind == 0 {
                        for index in 0..area {
                            scratch.predictor[index] =
                                ((webp_pixel(dst, index)? >> 8) & 0xff) as u8;
                        }
                    } else {
                        for index in 0..area {
                            let pixel = webp_pixel(dst, index)?;
                            scratch.cross_color[index * 3] = (pixel & 0xff) as u8;
                            scratch.cross_color[index * 3 + 1] = ((pixel >> 8) & 0xff) as u8;
                            scratch.cross_color[index * 3 + 2] = ((pixel >> 16) & 0xff) as u8;
                        }
                    }
                    transform.bits = block_bits as u8;
                    transform.data_w = data_w as u32;
                }
                2 => {}
                _ => {
                    let colors = bits.read(8)? as usize + 1;
                    let block_bits = webp_color_index_bits(colors);
                    let mut table = [0u8; 256 * 4];
                    webp_decode_argb(
                        bits,
                        colors,
                        1,
                        false,
                        scratch,
                        &mut table[..colors * 4],
                    )?;
                    let mut previous = webp_pixel(&table, 0)?;
                    scratch.palette[0] = previous;
                    for index in 1..colors {
                        previous = webp_add_pixels(previous, webp_pixel(&table, index)?);
                        scratch.palette[index] = previous;
                    }
                    transform.bits = block_bits as u8;
                    transform.colors = colors as u16;
                    image_width = webp_div_round_up(image_width, block_bits);
                }
            }
            transforms[transform_count] = transform;
            transform_count += 1;
        }
    }

    let cache_bits = if bits.read(1)? == 1 {        let cache_bits = bits.read(4)?;
        if cache_bits < 1 || cache_bits > WEBP_MAX_CACHE_BITS {
            return Err(());
        }
        cache_bits
    } else {
        0
    };
    let cache_size = if cache_bits == 0 { 0 } else { 1usize << cache_bits };

    let mut group_count = 1usize;
    let mut entropy_width = 0usize;
    let mut prefix_bits = 0u32;
    if level0 && bits.read(1)? == 1 {
        prefix_bits = bits.read(3)? + 2;
        entropy_width = webp_div_round_up(image_width, prefix_bits);
        let entropy_height = webp_div_round_up(height, prefix_bits);
        let area = entropy_width.checked_mul(entropy_height).ok_or(())?;
        if area > WEBP_MAX_ENTROPY_PIXELS {
            return Err(());
        }
        let bytes = area.checked_mul(4).ok_or(())?;
        if webp_decode_argb(
            bits,
            entropy_width,
            entropy_height,
            false,
            scratch,
            &mut dst[..bytes],
        )
        .is_err()
        {
            return Err(());
        }
        let mut largest = 0u16;
        for index in 0..area {
            let pixel = webp_pixel(dst, index)?;
            let group = ((pixel >> 8) & 0xffff) as u16;
            if group as usize >= WEBP_MAX_GROUPS {
                return Err(());
            }
            scratch.entropy[index] = group as u8;
            if group > largest {
                largest = group;
            }
        }
        group_count = largest as usize + 1;
    }

    let mut used = 0usize;
    for group in 0..group_count {
        let alphabets = [
            WEBP_GREEN_CODES + cache_size,
            WEBP_LITERAL_CODES,
            WEBP_LITERAL_CODES,
            WEBP_LITERAL_CODES,
            WEBP_DISTANCE_CODES,
        ];
        for (index, &alphabet) in alphabets.iter().enumerate() {
            let table = match webp_read_huffman(
                bits,
                &mut scratch.code_lengths[..alphabet],
                &mut scratch.symbols,
                &mut used,
            ) {
                Ok(table) => table,
                Err(()) => {
                    return Err(());
                }
            };
            scratch.groups[group][index] = table;
        }
    }

    if webp_decode_data(
        bits,
        image_width,
        height,
        cache_bits,
        prefix_bits,
        &scratch.groups[..group_count],
        &scratch.symbols,
        &scratch.entropy,
        entropy_width,
        &mut scratch.color_cache,
        dst,
    )
    .is_err()
    {
        return Err(());
    }

    let mut current_width = image_width;
    for index in (0..transform_count).rev() {
        let transform = transforms[index];
        match transform.kind {
            0 => {
                if current_width != transform.xsize as usize || height != transform.ysize as usize {
                    return Err(());
                }
                webp_inverse_predictor(
                    dst,
                    current_width,
                    height,
                    transform.bits as u32,
                    transform.data_w as usize,
                    &scratch.predictor,
                )?;
            }
            1 => {
                if current_width != transform.xsize as usize || height != transform.ysize as usize {
                    return Err(());
                }
                webp_inverse_cross_color(
                    dst,
                    current_width,
                    height,
                    transform.bits as u32,
                    transform.data_w as usize,
                    &scratch.cross_color,
                )?;
            }
            2 => webp_inverse_add_green(dst, total)?,
            _ => {
                if current_width
                    != webp_div_round_up(transform.xsize as usize, transform.bits as u32)
                    || height != transform.ysize as usize
                {
                    return Err(());
                }
                webp_inverse_color_index(
                    dst,
                    current_width,
                    height,
                    transform.xsize as usize,
                    transform.bits as u32,
                    &scratch.palette[..transform.colors as usize],
                )?;
                current_width = transform.xsize as usize;
            }
        }
    }
    if current_width != width {
        return Err(());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub enum WebpChunk<'a> {
    Lossless(&'a [u8]),
    Lossy(&'a [u8]),
}

/// Finds the image chunk of a WebP file and returns either its VP8L payload or
/// its lossy VP8 payload.
fn webp_image_chunk(data: &[u8]) -> Result<WebpChunk<'_>, ()> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(());
    }
    let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let end = 8usize.checked_add(riff_size).ok_or(())?;
    if riff_size < 4 || end > data.len() {
        return Err(());
    }
    let mut pos = 12usize;
    while pos.checked_add(8).ok_or(())? <= end {
        let size = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
            as usize;
        let start = pos + 8;
        let payload_end = start.checked_add(size).ok_or(())?;
        if payload_end > end {
            return Err(());
        }
        let kind = &data[pos..pos + 4];
        if kind == b"VP8L" {
            return Ok(WebpChunk::Lossless(&data[start..payload_end]));
        }
        if kind == b"VP8 " {
            return Ok(WebpChunk::Lossy(&data[start..payload_end]));
        }
        if kind == b"VP8X" {
            if size < 10 {
                return Err(());
            }
            let flags = data[start];
            // Refuse alpha (bit 4, 0x10) or animation (bit 1, 0x02)
            if (flags & 0x12) != 0 {
                return Err(());
            }
        }
        pos = payload_end.checked_add(size & 1).ok_or(())?;
    }
    Err(())
}

fn webp_vp8l_dimensions(chunk: &[u8]) -> Result<(u32, u32, bool), ()> {
    if chunk.len() < 5 || chunk[0] != 0x2F {
        return Err(());
    }
    let packed = u32::from_le_bytes([chunk[1], chunk[2], chunk[3], chunk[4]]);
    let width = (packed & 0x3fff) + 1;
    let height = ((packed >> 14) & 0x3fff) + 1;
    // The version field must be zero. Bit 28 says whether the stream carries
    // alpha; when it is clear every pixel is opaque and the decoder returns
    // alpha 255 regardless of the code stream.
    if packed >> 29 != 0 {
        return Err(());
    }
    if width > MAX_DECODE_DIMENSION || height > MAX_DECODE_DIMENSION {
        return Err(());
    }
    Ok((width, height, packed & (1 << 28) != 0))
}

/// Returns the dimensions a WebP file declares, without decoding pixels.
/// Supports both lossless (VP8L) and lossy (VP8 keyframe) streams.
pub fn decode_webp_header(data: &[u8]) -> Result<(u32, u32), ()> {
    match webp_image_chunk(data)? {
        WebpChunk::Lossless(chunk) => {
            let (width, height, _) = webp_vp8l_dimensions(chunk)?;
            Ok((width, height))
        }
        WebpChunk::Lossy(chunk) => {
            crate::vp8::dimensions(chunk)
        }
    }
}

/// Decodes a WebP file (lossless VP8L or lossy VP8 keyframe) into an RGBA
/// (4 bytes per pixel) destination buffer.
///
/// The plain RIFF form and the extended `VP8X` form are both read, including
/// files that carry ICC, Exif or XMP chunks ahead of the image. Returns the
/// dimensions and number of RGBA bytes written.
pub fn decode_webp(data: &[u8], out_rgba: &mut [u8]) -> Result<(u32, u32, usize), ()> {
    match webp_image_chunk(data)? {
        WebpChunk::Lossless(chunk) => {
            let (width, height, alpha_is_used) = webp_vp8l_dimensions(chunk)?;
            let pixels = (width as usize).checked_mul(height as usize).ok_or(())?;
            let required = pixels.checked_mul(4).ok_or(())?;
            if required > out_rgba.len() {
                return Err(());
            }

            let mut bits = WebpBits::new(&chunk[5..]);
            webp_decode_argb(
                &mut bits,
                width as usize,
                height as usize,
                true,
                webp_scratch(),
                out_rgba,
            )?;

            // The decoder works in ARGB words; the platform's pixels are red first.
            for index in 0..pixels {
                let offset = index.checked_mul(4).ok_or(())?;
                let end = offset.checked_add(4).ok_or(())?;
                let pixel = out_rgba.get_mut(offset..end).ok_or(())?;
                pixel.swap(0, 2);
                if !alpha_is_used {
                    pixel[3] = 255;
                }
            }
            Ok((width, height, required))
        }
        WebpChunk::Lossy(chunk) => {
            crate::vp8::decode(chunk, out_rgba)
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
    // An `.ico` is a directory rather than a picture, and its signature is
    // correspondingly weak -- two zero bytes, a type of one, and a count --
    // so it is only claimed when the header, the entry count and the first
    // entry's own bytes all line up. A file that merely starts with those
    // four bytes fails the walk and is refused, which is the same answer the
    // older code gave by not recognising it at all.
    if name_ends_with(name, b".ico") || data.len() >= 22 && data[..4] == [0, 0, 1, 0] {
        return decode_ico(data, out_rgba);
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
    // WHY JPEG is detected by three bytes and not two: SOI (`FF D8`) is
    // followed by a marker, so `FF D8 FF` is the honest signature; accepting a
    // bare `FF D8` would let any two-byte prefix claim the file. The name is
    // honoured too because a file manager may hand us a mislabelled document,
    // and `decode_jpeg` still refuses anything that is not a complete baseline
    // stream.
    if (data.len() >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF)
        || name_ends_with(name, b".jpg")
        || name_ends_with(name, b".jpeg")
    {
        return decode_jpeg(data, out_rgba);
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
    if (data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP")
        || name_ends_with(name, b".webp")
    {
        return decode_webp(data, out_rgba);
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
    use std::vec;
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

    struct GifTestFrame<'a> {
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        indices: &'a [u8],
        transparent: Option<u8>,
        disposal: u8,
        delay_cs: u16,
    }

    fn build_gif_animation(
        screen_w: u16,
        screen_h: u16,
        loop_count: Option<u16>,
        frames: &[GifTestFrame<'_>],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&screen_w.to_le_bytes());
        out.extend_from_slice(&screen_h.to_le_bytes());
        out.push(0x80 | 0x01); // global colour table, 4 entries
        out.push(0);
        out.push(0);
        for entry in GIF_PALETTE {
            out.extend_from_slice(&entry);
        }
        if let Some(loops) = loop_count {
            out.extend_from_slice(&[0x21, 0xFF, 0x0B]);
            out.extend_from_slice(b"NETSCAPE2.0");
            out.extend_from_slice(&[0x03, 0x01]);
            out.extend_from_slice(&loops.to_le_bytes());
            out.push(0x00);
        }
        for frame in frames {
            let transparent_flag = if frame.transparent.is_some() { 1 } else { 0 };
            out.extend_from_slice(&[
                0x21,
                0xF9,
                0x04,
                (frame.disposal << 2) | transparent_flag,
                (frame.delay_cs & 0xFF) as u8,
                (frame.delay_cs >> 8) as u8,
                frame.transparent.unwrap_or(0),
                0x00,
            ]);
            out.push(0x2C);
            out.extend_from_slice(&frame.left.to_le_bytes());
            out.extend_from_slice(&frame.top.to_le_bytes());
            out.extend_from_slice(&frame.width.to_le_bytes());
            out.extend_from_slice(&frame.height.to_le_bytes());
            out.push(0x00);
            out.push(2); // LZW minimum code size
            let lzw = gif_lzw_literals(frame.indices, 2);
            for block in lzw.chunks(255) {
                out.push(block.len() as u8);
                out.extend_from_slice(block);
            }
            out.push(0);
        }
        out.push(0x3B);
        out
    }

    fn frame(
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        indices: &[u8],
        transparent: Option<u8>,
        disposal: u8,
        delay_cs: u16,
    ) -> GifTestFrame<'_> {
        GifTestFrame {
            left,
            top,
            width,
            height,
            indices,
            transparent,
            disposal,
            delay_cs,
        }
    }

    #[test]
    fn gif_animation_reports_frames_and_looping() {
        let gif = build_gif_animation(
            2,
            1,
            Some(0),
            &[
                frame(0, 0, 2, 1, &[1, 1], None, 1, 5),
                frame(0, 0, 2, 1, &[1, 1], None, 1, 7),
            ],
        );
        let animation = GifAnimation::new(&gif).unwrap();
        assert_eq!(
            animation.info(),
            GifAnimationInfo {
                width: 2,
                height: 1,
                frame_count: 2,
                loop_count: 0,
            }
        );
        assert_eq!(animation.dimensions(), (2, 1));

        let still = build_gif(1, 1, None, 0, 0, 1, 1, &[1], false);
        assert_eq!(
            gif_animation_info(&still),
            Ok(GifAnimationInfo {
                width: 1,
                height: 1,
                frame_count: 1,
                loop_count: 1,
            })
        );
    }

    #[test]
    fn gif_animation_composites_frames_and_reports_delay() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(
            2,
            1,
            None,
            &[
                frame(0, 0, 2, 1, &[1, 1], None, 1, 5),
                frame(0, 0, 2, 1, &[2, 2], None, 1, 7),
            ],
        );
        let mut animation = GifAnimation::new(&gif).unwrap();
        let mut canvas = [0u8; 8];
        let mut restore = [0u8; 8];

        let first = animation.decode_next(&mut canvas, &mut restore).unwrap();
        assert_eq!(first.delay_cs, 5);
        assert_eq!(first.disposal, 1);
        assert_eq!(&canvas, &[255, 0, 0, 255, 255, 0, 0, 255]);
        assert_eq!(animation.frame_index(), 1);

        let second = animation.decode_next(&mut canvas, &mut restore).unwrap();
        assert_eq!(second.delay_cs, 7);
        assert_eq!(&canvas, &[0, 255, 0, 255, 0, 255, 0, 255]);
        assert_eq!(animation.frame_index(), 2);
    }

    #[test]
    fn gif_animation_keeps_the_canvas_under_transparent_pixels() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(
            2,
            1,
            None,
            &[
                frame(0, 0, 2, 1, &[1, 1], None, 1, 0),
                frame(0, 0, 2, 1, &[1, 2], Some(1), 1, 0),
            ],
        );
        let mut animation = GifAnimation::new(&gif).unwrap();
        let mut canvas = [0u8; 8];
        let mut restore = [0u8; 8];
        animation.decode_next(&mut canvas, &mut restore).unwrap();
        animation.decode_next(&mut canvas, &mut restore).unwrap();
        assert_eq!(
            &canvas,
            &[255, 0, 0, 255, 0, 255, 0, 255],
            "the transparent pixel leaves the previous frame's colour in place"
        );
    }

    #[test]
    fn gif_disposal_two_clears_the_frame_rectangle() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(
            2,
            1,
            None,
            &[
                frame(0, 0, 2, 1, &[1, 1], None, 2, 0),
                frame(0, 0, 1, 1, &[2], None, 1, 0),
            ],
        );
        let mut animation = GifAnimation::new(&gif).unwrap();
        let mut canvas = [0u8; 8];
        let mut restore = [0u8; 8];
        animation.decode_next(&mut canvas, &mut restore).unwrap();
        animation.decode_next(&mut canvas, &mut restore).unwrap();
        assert_eq!(
            &canvas,
            &[0, 255, 0, 255, 0, 0, 0, 0],
            "the first frame's rectangle is cleared before the second is drawn"
        );
    }

    #[test]
    fn gif_disposal_three_restores_the_previous_rectangle() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(
            2,
            1,
            None,
            &[
                frame(0, 0, 2, 1, &[1, 1], None, 3, 0),
                frame(0, 0, 2, 1, &[0, 0], Some(0), 1, 0),
            ],
        );
        let mut animation = GifAnimation::new(&gif).unwrap();
        // The canvas starts blue, so restoring the saved rectangle is
        // observable: frame one paints red, frame two restores blue and then
        // paints nothing because every pixel is transparent.
        let mut canvas = [0u8, 0, 255, 255, 0, 0, 255, 255];
        let mut restore = [0u8; 8];
        animation.decode_next(&mut canvas, &mut restore).unwrap();
        assert_eq!(&canvas, &[255, 0, 0, 255, 255, 0, 0, 255]);
        animation.decode_next(&mut canvas, &mut restore).unwrap();
        assert_eq!(&canvas, &[0, 0, 255, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn gif_disposal_three_requires_a_restore_buffer_for_the_frame() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(
            2,
            1,
            None,
            &[frame(0, 0, 2, 1, &[1, 1], None, 3, 0)],
        );
        let mut animation = GifAnimation::new(&gif).unwrap();
        let mut canvas = [0u8; 8];
        let mut small = [0u8; 4];
        assert!(animation.decode_next(&mut canvas, &mut small).is_err());
    }

    #[test]
    fn gif_animation_finishes_at_the_trailer_and_rewinds() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(1, 1, None, &[frame(0, 0, 1, 1, &[1], None, 1, 3)]);
        let mut animation = GifAnimation::new(&gif).unwrap();
        let mut canvas = [0u8; 4];
        let mut restore = [0u8; 4];
        assert!(animation.decode_next(&mut canvas, &mut restore).is_ok());
        assert!(!animation.is_finished());
        assert!(animation.decode_next(&mut canvas, &mut restore).is_err());
        assert!(animation.is_finished());

        animation.rewind();
        assert!(!animation.is_finished());
        assert_eq!(animation.frame_index(), 0);
        assert!(animation.decode_next(&mut canvas, &mut restore).is_ok());
    }

    #[test]
    fn gif_animation_refuses_a_canvas_that_is_not_the_screen() {
        let _guard = scratch_guard();
        let gif = build_gif_animation(2, 2, None, &[frame(0, 0, 2, 2, &[1; 4], None, 1, 0)]);
        let mut animation = GifAnimation::new(&gif).unwrap();
        let mut small = [0u8; 15];
        let mut restore = [0u8; 16];
        assert!(animation.decode_next(&mut small, &mut restore).is_err());
    }

    // -- JPEG fixtures -----------------------------------------------------
    //
    // Generated once with Pillow 12 and embedded here: the suite must not
    // depend on a fixture directory. The grayscale image uses quality 100,
    // whose quantization tables are all ones, so a flat block reconstructs to
    // exactly the value that was stored. The colour images cover 4:4:4, 4:2:2
    // and 4:2:0 sampling; the restart image carries DRI=1, so its two MCUs are
    // separated by an RST0 marker.

    const JPEG_GRAY_8X8: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1,
        0, 1, 0, 0, 255, 219, 0, 67, 0, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 255, 192, 0, 11, 8, 0, 8,
        0, 8, 1, 1, 17, 0, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1,
        1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4,
        5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3,
        3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0,
        4, 17, 5, 18, 33, 49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50,
        129, 145, 161, 8, 35, 66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114,
        130, 9, 10, 22, 23, 24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53,
        54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85,
        86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117,
        118, 119, 120, 121, 122, 131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148,
        149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178,
        179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201,
        202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230,
        231, 232, 233, 234, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 218,
        0, 8, 1, 1, 0, 0, 63, 0, 43, 255, 217,
    ];

    const JPEG_COLOR_16X16_420: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1,
        0, 1, 0, 0, 255, 219, 0, 67, 0, 2, 1, 1, 1, 1, 1, 2,
        1, 1, 1, 2, 2, 2, 2, 2, 4, 3, 2, 2, 2, 2, 5, 4,
        4, 3, 4, 6, 5, 6, 6, 6, 5, 6, 6, 6, 7, 9, 8, 6,
        7, 9, 7, 6, 6, 8, 11, 8, 9, 10, 10, 10, 10, 10, 6, 8,
        11, 12, 11, 10, 12, 9, 10, 10, 10, 255, 219, 0, 67, 1, 2, 2,
        2, 2, 2, 2, 5, 3, 3, 5, 10, 7, 6, 7, 10, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 255, 192,
        0, 17, 8, 0, 16, 0, 16, 3, 1, 34, 0, 2, 17, 1, 3, 17,
        1, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0,
        0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
        10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3, 3, 2, 4, 3, 5,
        5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33,
        49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50, 129, 145, 161, 8, 35,
        66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114, 130, 9, 10, 22, 23,
        24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56, 57, 58,
        67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90,
        99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122,
        131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153,
        154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183,
        184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213,
        214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241,
        242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 196, 0, 31, 1, 0, 3,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1,
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 17, 0,
        2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119, 0,
        1, 2, 3, 17, 4, 5, 33, 49, 6, 18, 65, 81, 7, 97, 113, 19,
        34, 50, 129, 8, 20, 66, 145, 161, 177, 193, 9, 35, 51, 82, 240, 21,
        98, 114, 209, 10, 22, 36, 52, 225, 37, 241, 23, 24, 25, 26, 38, 39,
        40, 41, 42, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73,
        74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105,
        106, 115, 116, 117, 118, 119, 120, 121, 122, 130, 131, 132, 133, 134, 135, 136,
        137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166,
        167, 168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196,
        197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 226,
        227, 228, 229, 230, 231, 232, 233, 234, 242, 243, 244, 245, 246, 247, 248, 249,
        250, 255, 218, 0, 12, 3, 1, 0, 2, 17, 3, 17, 0, 63, 0, 249,
        222, 190, 168, 175, 130, 235, 247, 226, 191, 39, 250, 87, 112, 87, 252, 75,
        191, 246, 63, 239, 254, 189, 245, 239, 172, 125, 159, 99, 201, 236, 125, 135,
        247, 170, 243, 115, 123, 95, 238, 242, 242, 245, 190, 158, 191, 141, 217, 143,
        252, 77, 23, 212, 61, 223, 236, 255, 0, 236, 255, 0, 107, 215, 235, 30,
        211, 235, 30, 207, 202, 143, 39, 39, 177, 254, 247, 55, 55, 217, 229, 215,
        255, 217,
    ];

    const JPEG_COLOR_32X16_RESTART: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1,
        0, 1, 0, 0, 255, 219, 0, 67, 0, 2, 1, 1, 1, 1, 1, 2,
        1, 1, 1, 2, 2, 2, 2, 2, 4, 3, 2, 2, 2, 2, 5, 4,
        4, 3, 4, 6, 5, 6, 6, 6, 5, 6, 6, 6, 7, 9, 8, 6,
        7, 9, 7, 6, 6, 8, 11, 8, 9, 10, 10, 10, 10, 10, 6, 8,
        11, 12, 11, 10, 12, 9, 10, 10, 10, 255, 219, 0, 67, 1, 2, 2,
        2, 2, 2, 2, 5, 3, 3, 5, 10, 7, 6, 7, 10, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
        10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 255, 192,
        0, 17, 8, 0, 16, 0, 32, 3, 1, 34, 0, 2, 17, 1, 3, 17,
        1, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0,
        0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
        10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3, 3, 2, 4, 3, 5,
        5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33,
        49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50, 129, 145, 161, 8, 35,
        66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114, 130, 9, 10, 22, 23,
        24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56, 57, 58,
        67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90,
        99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122,
        131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153,
        154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183,
        184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213,
        214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241,
        242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 196, 0, 31, 1, 0, 3,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1,
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 17, 0,
        2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119, 0,
        1, 2, 3, 17, 4, 5, 33, 49, 6, 18, 65, 81, 7, 97, 113, 19,
        34, 50, 129, 8, 20, 66, 145, 161, 177, 193, 9, 35, 51, 82, 240, 21,
        98, 114, 209, 10, 22, 36, 52, 225, 37, 241, 23, 24, 25, 26, 38, 39,
        40, 41, 42, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73,
        74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105,
        106, 115, 116, 117, 118, 119, 120, 121, 122, 130, 131, 132, 133, 134, 135, 136,
        137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166,
        167, 168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196,
        197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 226,
        227, 228, 229, 230, 231, 232, 233, 234, 242, 243, 244, 245, 246, 247, 248, 249,
        250, 255, 221, 0, 4, 0, 1, 255, 218, 0, 12, 3, 1, 0, 2, 17,
        3, 17, 0, 63, 0, 239, 40, 162, 138, 255, 0, 63, 207, 239, 131, 255,
        208, 199, 162, 138, 43, 251, 112, 255, 0, 61, 207, 255, 217,
    ];

    const JPEG_COLOR_8X8_444: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1,
        0, 1, 0, 0, 255, 219, 0, 67, 0, 3, 2, 2, 3, 2, 2, 3,
        3, 3, 3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7,
        7, 6, 8, 12, 10, 12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13,
        14, 17, 14, 11, 11, 16, 22, 16, 17, 19, 20, 21, 21, 21, 12, 15,
        23, 24, 22, 20, 24, 18, 20, 21, 20, 255, 219, 0, 67, 1, 3, 4,
        4, 5, 4, 5, 9, 5, 5, 9, 20, 13, 11, 13, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 255, 192,
        0, 17, 8, 0, 8, 0, 8, 3, 1, 17, 0, 2, 17, 1, 3, 17,
        1, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0,
        0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
        10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3, 3, 2, 4, 3, 5,
        5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33,
        49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50, 129, 145, 161, 8, 35,
        66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114, 130, 9, 10, 22, 23,
        24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56, 57, 58,
        67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90,
        99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122,
        131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153,
        154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183,
        184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213,
        214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241,
        242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 196, 0, 31, 1, 0, 3,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1,
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 17, 0,
        2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119, 0,
        1, 2, 3, 17, 4, 5, 33, 49, 6, 18, 65, 81, 7, 97, 113, 19,
        34, 50, 129, 8, 20, 66, 145, 161, 177, 193, 9, 35, 51, 82, 240, 21,
        98, 114, 209, 10, 22, 36, 52, 225, 37, 241, 23, 24, 25, 26, 38, 39,
        40, 41, 42, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73,
        74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105,
        106, 115, 116, 117, 118, 119, 120, 121, 122, 130, 131, 132, 133, 134, 135, 136,
        137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166,
        167, 168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196,
        197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 226,
        227, 228, 229, 230, 231, 232, 233, 234, 242, 243, 244, 245, 246, 247, 248, 249,
        250, 255, 218, 0, 12, 3, 1, 0, 2, 17, 3, 17, 0, 63, 0, 169,
        95, 135, 159, 210, 7, 255, 217,
    ];

    const JPEG_COLOR_8X8_422: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1,
        0, 1, 0, 0, 255, 219, 0, 67, 0, 3, 2, 2, 3, 2, 2, 3,
        3, 3, 3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7,
        7, 6, 8, 12, 10, 12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13,
        14, 17, 14, 11, 11, 16, 22, 16, 17, 19, 20, 21, 21, 21, 12, 15,
        23, 24, 22, 20, 24, 18, 20, 21, 20, 255, 219, 0, 67, 1, 3, 4,
        4, 5, 4, 5, 9, 5, 5, 9, 20, 13, 11, 13, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 255, 192,
        0, 17, 8, 0, 8, 0, 8, 3, 1, 33, 0, 2, 17, 1, 3, 17,
        1, 255, 196, 0, 31, 0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0,
        0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
        10, 11, 255, 196, 0, 181, 16, 0, 2, 1, 3, 3, 2, 4, 3, 5,
        5, 4, 4, 0, 0, 1, 125, 1, 2, 3, 0, 4, 17, 5, 18, 33,
        49, 65, 6, 19, 81, 97, 7, 34, 113, 20, 50, 129, 145, 161, 8, 35,
        66, 177, 193, 21, 82, 209, 240, 36, 51, 98, 114, 130, 9, 10, 22, 23,
        24, 25, 26, 37, 38, 39, 40, 41, 42, 52, 53, 54, 55, 56, 57, 58,
        67, 68, 69, 70, 71, 72, 73, 74, 83, 84, 85, 86, 87, 88, 89, 90,
        99, 100, 101, 102, 103, 104, 105, 106, 115, 116, 117, 118, 119, 120, 121, 122,
        131, 132, 133, 134, 135, 136, 137, 138, 146, 147, 148, 149, 150, 151, 152, 153,
        154, 162, 163, 164, 165, 166, 167, 168, 169, 170, 178, 179, 180, 181, 182, 183,
        184, 185, 186, 194, 195, 196, 197, 198, 199, 200, 201, 202, 210, 211, 212, 213,
        214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241,
        242, 243, 244, 245, 246, 247, 248, 249, 250, 255, 196, 0, 31, 1, 0, 3,
        1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1,
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 17, 0,
        2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119, 0,
        1, 2, 3, 17, 4, 5, 33, 49, 6, 18, 65, 81, 7, 97, 113, 19,
        34, 50, 129, 8, 20, 66, 145, 161, 177, 193, 9, 35, 51, 82, 240, 21,
        98, 114, 209, 10, 22, 36, 52, 225, 37, 241, 23, 24, 25, 26, 38, 39,
        40, 41, 42, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73,
        74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105,
        106, 115, 116, 117, 118, 119, 120, 121, 122, 130, 131, 132, 133, 134, 135, 136,
        137, 138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166,
        167, 168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196,
        197, 198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 226,
        227, 228, 229, 230, 231, 232, 233, 234, 242, 243, 244, 245, 246, 247, 248, 249,
        250, 255, 218, 0, 12, 3, 1, 0, 2, 17, 3, 17, 0, 63, 0, 169,
        69, 126, 30, 127, 72, 31, 255, 217,
    ];

    const JPEG_RGB_8X8_ADOBE: &[u8] = &[
        255, 216, 255, 238, 0, 14, 65, 100, 111, 98, 101, 0, 100, 0, 0, 0,
        0, 0, 255, 219, 0, 67, 0, 3, 2, 2, 3, 2, 2, 3, 3, 3,
        3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7, 7, 6,
        8, 12, 10, 12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13, 14, 17,
        14, 11, 11, 16, 22, 16, 17, 19, 20, 21, 21, 21, 12, 15, 23, 24,
        22, 20, 24, 18, 20, 21, 20, 255, 192, 0, 17, 8, 0, 8, 0, 8,
        3, 82, 17, 0, 71, 17, 0, 66, 17, 0, 255, 196, 0, 31, 0, 0,
        1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 16,
        0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125,
        1, 2, 3, 0, 4, 17, 5, 18, 33, 49, 65, 6, 19, 81, 97, 7,
        34, 113, 20, 50, 129, 145, 161, 8, 35, 66, 177, 193, 21, 82, 209, 240,
        36, 51, 98, 114, 130, 9, 10, 22, 23, 24, 25, 26, 37, 38, 39, 40,
        41, 42, 52, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73,
        74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105,
        106, 115, 116, 117, 118, 119, 120, 121, 122, 131, 132, 133, 134, 135, 136, 137,
        138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167,
        168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197,
        198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 225, 226,
        227, 228, 229, 230, 231, 232, 233, 234, 241, 242, 243, 244, 245, 246, 247, 248,
        249, 250, 255, 218, 0, 12, 3, 82, 0, 71, 0, 66, 0, 0, 63, 0,
        251, 2, 188, 210, 190, 47, 175, 255, 217,
    ];

    const JPEG_RGB_8X8_IDS: &[u8] = &[
        255, 216, 255, 219, 0, 67, 0, 3, 2, 2, 3, 2, 2, 3, 3, 3,
        3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7, 7, 6,
        8, 12, 10, 12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13, 14, 17,
        14, 11, 11, 16, 22, 16, 17, 19, 20, 21, 21, 21, 12, 15, 23, 24,
        22, 20, 24, 18, 20, 21, 20, 255, 192, 0, 17, 8, 0, 8, 0, 8,
        3, 82, 17, 0, 71, 17, 0, 66, 17, 0, 255, 196, 0, 31, 0, 0,
        1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 255, 196, 0, 181, 16,
        0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125,
        1, 2, 3, 0, 4, 17, 5, 18, 33, 49, 65, 6, 19, 81, 97, 7,
        34, 113, 20, 50, 129, 145, 161, 8, 35, 66, 177, 193, 21, 82, 209, 240,
        36, 51, 98, 114, 130, 9, 10, 22, 23, 24, 25, 26, 37, 38, 39, 40,
        41, 42, 52, 53, 54, 55, 56, 57, 58, 67, 68, 69, 70, 71, 72, 73,
        74, 83, 84, 85, 86, 87, 88, 89, 90, 99, 100, 101, 102, 103, 104, 105,
        106, 115, 116, 117, 118, 119, 120, 121, 122, 131, 132, 133, 134, 135, 136, 137,
        138, 146, 147, 148, 149, 150, 151, 152, 153, 154, 162, 163, 164, 165, 166, 167,
        168, 169, 170, 178, 179, 180, 181, 182, 183, 184, 185, 186, 194, 195, 196, 197,
        198, 199, 200, 201, 202, 210, 211, 212, 213, 214, 215, 216, 217, 218, 225, 226,
        227, 228, 229, 230, 231, 232, 233, 234, 241, 242, 243, 244, 245, 246, 247, 248,
        249, 250, 255, 218, 0, 12, 3, 82, 0, 71, 0, 66, 0, 0, 63, 0,
        251, 2, 188, 210, 190, 47, 175, 255, 217,
    ];

    const JPEG_PROGRESSIVE_8X8_GRAY: &[u8] = &[
        255, 216, 255, 224, 0, 16, 74, 70, 73, 70, 0, 1, 1, 0, 0, 1,
        0, 1, 0, 0, 255, 219, 0, 67, 0, 3, 2, 2, 3, 2, 2, 3,
        3, 3, 3, 4, 3, 3, 4, 5, 8, 5, 5, 4, 4, 5, 10, 7,
        7, 6, 8, 12, 10, 12, 12, 11, 10, 11, 11, 13, 14, 18, 16, 13,
        14, 17, 14, 11, 11, 16, 22, 16, 17, 19, 20, 21, 21, 21, 12, 15,
        23, 24, 22, 20, 24, 18, 20, 21, 20, 255, 194, 0, 11, 8, 0, 8,
        0, 8, 1, 1, 17, 0, 255, 196, 0, 20, 0, 1, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 218, 0, 8,
        1, 1, 0, 0, 0, 1, 127, 255, 196, 0, 20, 16, 1, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 218, 0,
        8, 1, 1, 0, 1, 5, 2, 127, 255, 196, 0, 20, 16, 1, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 218,
        0, 8, 1, 1, 0, 6, 63, 2, 127, 255, 196, 0, 20, 16, 1, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255,
        218, 0, 8, 1, 1, 0, 1, 63, 33, 127, 255, 218, 0, 8, 1, 1,
        0, 0, 0, 16, 127, 255, 196, 0, 20, 16, 1, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 218, 0, 8, 1,
        1, 0, 1, 63, 16, 127, 255, 217,
    ];

    fn jpeg_pixel(out: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
        let offset = (y * width + x) * 4;
        [out[offset], out[offset + 1], out[offset + 2]]
    }

    fn jpeg_close(actual: [u8; 3], expected: [u8; 3]) -> bool {
        actual
            .iter()
            .zip(expected.iter())
            .all(|(a, e)| (*a as i32 - *e as i32).abs() <= 24)
    }

    #[test]
    fn jpeg_grayscale_8x8_decodes_to_uniform_grey() {
        let _guard = scratch_guard();
        assert_eq!(decode_jpeg_header(JPEG_GRAY_8X8), Ok((8, 8)));
        let mut out = [0u8; 8 * 8 * 4];
        assert_eq!(decode_jpeg(JPEG_GRAY_8X8, &mut out), Ok((8, 8, 256)));
        for pixel in out.chunks_exact(4) {
            assert_eq!(pixel, &[128, 128, 128, 255]);
        }
    }

    #[test]
    fn jpeg_color_16x16_420_decodes_plausible_rgb() {
        let _guard = scratch_guard();
        assert_eq!(decode_jpeg_header(JPEG_COLOR_16X16_420), Ok((16, 16)));
        let mut out = [0u8; 16 * 16 * 4];
        assert_eq!(
            decode_jpeg(JPEG_COLOR_16X16_420, &mut out),
            Ok((16, 16, 1024))
        );
        // Every quadrant is uniform and aligns with the chroma grid, so the
        // decoded colours stay close to what Pillow stored.
        assert!(jpeg_close(jpeg_pixel(&out, 16, 4, 4), [220, 40, 40]));
        assert!(jpeg_close(jpeg_pixel(&out, 16, 12, 4), [40, 200, 60]));
        assert!(jpeg_close(jpeg_pixel(&out, 16, 4, 12), [50, 60, 220]));
        assert!(jpeg_close(jpeg_pixel(&out, 16, 12, 12), [230, 220, 30]));
        assert_eq!(out[3], 255);
    }

    #[test]
    fn jpeg_422_and_444_sampling_decode() {
        let _guard = scratch_guard();
        let mut out = [0u8; 8 * 8 * 4];
        assert_eq!(decode_jpeg(JPEG_COLOR_8X8_444, &mut out), Ok((8, 8, 256)));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 0, 0), [200, 100, 50]));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 7, 7), [200, 100, 50]));

        let mut out = [0u8; 8 * 8 * 4];
        assert_eq!(decode_jpeg(JPEG_COLOR_8X8_422, &mut out), Ok((8, 8, 256)));
        // 4:2:2 halves the chroma only horizontally; the picture is one flat
        // colour, so nearest-neighbour replication must still land on it.
        assert!(jpeg_close(jpeg_pixel(&out, 8, 0, 0), [200, 100, 50]));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 7, 7), [200, 100, 50]));
    }

    #[test]
    fn jpeg_rgb_components_skip_the_ycbcr_transform() {
        let _guard = scratch_guard();

        // Pillow's keep_rgb encoder writes an Adobe APP14 with transform 0.
        let mut out = [0u8; 8 * 8 * 4];
        assert_eq!(decode_jpeg(JPEG_RGB_8X8_ADOBE, &mut out), Ok((8, 8, 256)));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 0, 0), [200, 100, 50]));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 7, 7), [200, 100, 50]));

        // The same stream with APP14 removed is identified by its R/G/B IDs.
        let mut out = [0u8; 8 * 8 * 4];
        assert_eq!(decode_jpeg(JPEG_RGB_8X8_IDS, &mut out), Ok((8, 8, 256)));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 0, 0), [200, 100, 50]));
        assert!(jpeg_close(jpeg_pixel(&out, 8, 7, 7), [200, 100, 50]));
    }

    #[test]
    fn jpeg_restart_markers_are_consumed() {
        let _guard = scratch_guard();
        let mut out = [0u8; 32 * 16 * 4];
        assert_eq!(
            decode_jpeg(JPEG_COLOR_32X16_RESTART, &mut out),
            Ok((32, 16, 2048))
        );
        // MCU 0 is orange, MCU 1 is blue; RST0 sits between them.
        assert!(jpeg_close(jpeg_pixel(&out, 32, 8, 8), [230, 121, 30]));
        assert!(jpeg_close(jpeg_pixel(&out, 32, 24, 8), [29, 150, 219]));
        assert!(jpeg_close(jpeg_pixel(&out, 32, 0, 0), [230, 121, 30]));
        assert!(jpeg_close(jpeg_pixel(&out, 32, 31, 15), [29, 150, 219]));
    }

    #[test]
    fn jpeg_truncated_stream_is_refused() {
        let _guard = scratch_guard();
        let mut out = [0u8; 8 * 8 * 4];
        // Cut before the scan: the header walk cannot reach SOS.
        assert!(decode_jpeg(&JPEG_GRAY_8X8[..JPEG_GRAY_8X8.len() / 2], &mut out).is_err());
        assert!(decode_jpeg(&JPEG_GRAY_8X8[..10], &mut out).is_err());
        assert!(decode_jpeg(b"not a jpeg", &mut out).is_err());

        // Cut inside the entropy-coded data: the last MCU is incomplete, so
        // the decoder must refuse rather than leave part of the surface
        // unpainted.
        let mut out = [0u8; 16 * 16 * 4];
        assert!(decode_jpeg(
            &JPEG_COLOR_16X16_420[..JPEG_COLOR_16X16_420.len() - 10],
            &mut out
        )
        .is_err());

        // The final MCU is present but the RST0 that should precede it (bytes
        // 639..641 of the fixture) was removed. That is malformed, not a
        // complete picture.
        let mut without_restart = JPEG_COLOR_32X16_RESTART.to_vec();
        without_restart.drain(639..641);
        let mut out = [0u8; 32 * 16 * 4];
        assert!(decode_jpeg(&without_restart, &mut out).is_err());
    }

    #[test]
    fn jpeg_progressive_is_refused() {
        let _guard = scratch_guard();
        let mut out = [0u8; 8 * 8 * 4];
        assert!(decode_jpeg(JPEG_PROGRESSIVE_8X8_GRAY, &mut out).is_err());
        assert!(decode_jpeg_header(JPEG_PROGRESSIVE_8X8_GRAY).is_err());
    }

    #[test]
    fn jpeg_output_buffer_too_small_is_refused() {
        let _guard = scratch_guard();
        let mut small = [0u8; 8 * 8 * 4 - 1];
        assert!(decode_jpeg(JPEG_GRAY_8X8, &mut small).is_err());
        let mut out = [0u8; 8 * 8 * 4];
        assert!(decode_image(b"not a jpeg", b"photo.jpg", &mut out).is_err());
    }

    #[test]
    fn jpeg_header_reports_dimensions_only_for_a_decodable_frame() {
        assert_eq!(decode_jpeg_header(JPEG_GRAY_8X8), Ok((8, 8)));
        assert_eq!(decode_jpeg_header(JPEG_COLOR_16X16_420), Ok((16, 16)));
        assert!(decode_jpeg_header(&JPEG_GRAY_8X8[..8]).is_err());
        assert!(decode_jpeg_header(b"not a jpeg").is_err());
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

    #[test]
    fn decode_image_routes_jpeg_by_signature_and_suffix() {
        let _guard = scratch_guard();
        let mut out = [0u8; 8 * 8 * 4];

        // The signature decides even when the name says nothing.
        assert_eq!(
            decode_image(JPEG_GRAY_8X8, b"mystery", &mut out),
            Ok((8, 8, 256))
        );
        assert_eq!(&out[..4], &[128, 128, 128, 255]);

        // A recognised suffix selects the decoder too, and the decoder itself
        // still refuses data that is not a JPEG.
        assert_eq!(
            decode_image(JPEG_GRAY_8X8, b"photo.JPG", &mut out),
            Ok((8, 8, 256))
        );
        assert_eq!(
            decode_image(JPEG_COLOR_16X16_420, b"photo.jpeg", &mut [0u8; 1024]),
            Ok((16, 16, 1024))
        );
        assert!(decode_image(b"not a jpeg", b"photo.jpg", &mut out).is_err());
    }

    // -- WebP builders -----------------------------------------------------

    /// A least-significant-bit-first writer for synthetic VP8L streams.
    struct Vp8lWriter {
        bytes: Vec<u8>,
        bit: u32,
    }

    impl Vp8lWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                bit: 0,
            }
        }

        fn put(&mut self, value: u32, count: u32) {
            for index in 0..count {
                if self.bit == 0 {
                    self.bytes.push(0);
                }
                if (value >> index) & 1 != 0 {
                    let last = self.bytes.len() - 1;
                    self.bytes[last] |= 1 << self.bit;
                }
                self.bit = (self.bit + 1) & 7;
            }
        }
    }

    /// A whole VP8L payload: the five-byte header followed by the bitstream.
    fn vp8l_stream(width: u32, height: u32, alpha_hint: u32, body: Vp8lWriter) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0x2F);
        let packed = (width - 1) | ((height - 1) << 14) | (alpha_hint << 28);
        out.extend_from_slice(&packed.to_le_bytes());
        out.extend_from_slice(&body.bytes);
        out
    }

    /// A RIFF/WEBP container around the given chunks.
    fn riff_file(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"WEBP");
        for (kind, payload) in chunks {
            body.extend_from_slice(*kind);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
            if payload.len() % 2 != 0 {
                body.push(0);
            }
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// A simple prefix code naming one or two symbols of length one.
    fn put_simple_code(writer: &mut Vp8lWriter, symbols: &[u16]) {
        writer.put(1, 1);
        writer.put(if symbols.len() == 2 { 1 } else { 0 }, 1);
        if symbols[0] > 1 {
            writer.put(1, 1);
            writer.put(symbols[0] as u32, 8);
        } else {
            writer.put(0, 1);
            writer.put(symbols[0] as u32, 1);
        }
        if symbols.len() == 2 {
            writer.put(symbols[1] as u32, 8);
        }
    }

    /// A normal prefix code with two symbols of length one, reached through a
    /// two-symbol code-length code.
    fn put_two_symbol_code(writer: &mut Vp8lWriter, alphabet: usize, symbols: [u16; 2]) {
        writer.put(0, 1); // normal code
        writer.put(0, 4); // four code-length codes
        writer.put(0, 3); // 17: absent
        writer.put(0, 3); // 18: absent
        writer.put(1, 3); // 0: length one
        writer.put(1, 3); // 1: length one
        writer.put(0, 1); // max_symbol is the whole alphabet
        for symbol in 0..alphabet {
            let value = usize::from(symbols[0] as usize == symbol || symbols[1] as usize == symbol);
            writer.put(value as u32, 1);
        }
    }

    fn put_group(
        writer: &mut Vp8lWriter,
        green: &[u16],
        red: &[u16],
        blue: &[u16],
        alpha: &[u16],
        distance: &[u16],
    ) {
        put_simple_code(writer, green);
        put_simple_code(writer, red);
        put_simple_code(writer, blue);
        put_simple_code(writer, alpha);
        put_simple_code(writer, distance);
    }

    /// The main image header: no transforms, no cache, one prefix-code group.
    fn put_stream_start(writer: &mut Vp8lWriter) {
        writer.put(0, 1);
        writer.put(0, 1);
        writer.put(0, 1);
    }

    fn webp_one_pixel(green: u16, red: u16, blue: u16, alpha: u16, hint: u32) -> Vec<u8> {
        let mut body = Vp8lWriter::new();
        put_stream_start(&mut body);
        put_group(&mut body, &[green], &[red], &[blue], &[alpha], &[0]);
        vp8l_stream(1, 1, hint, body)
    }

    /// A plain RIFF/WEBP file holding one lossless image.
    fn webp_file(image: &[u8]) -> Vec<u8> {
        riff_file(&[(b"VP8L", image)])
    }

    #[test]
    fn webp_lossless_decodes_a_single_pixel_literal() {
        let _guard = scratch_guard();
        let stream = webp_file(&webp_one_pixel(0x40, 0x20, 0x10, 0xFF, 0));
        assert_eq!(decode_webp_header(&stream), Ok((1, 1)));
        let mut out = [0u8; 4];
        assert_eq!(decode_webp(&stream, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[0x20, 0x40, 0x10, 0xFF]);
    }

    #[test]
    fn webp_lossless_marks_pixels_opaque_when_the_alpha_hint_is_clear() {
        let _guard = scratch_guard();
        // The code stream says alpha zero, but the header declares that the
        // image does not use alpha, so the output must still be opaque.
        let stream = webp_file(&webp_one_pixel(1, 2, 3, 0, 0));
        let mut out = [0u8; 4];
        assert_eq!(decode_webp(&stream, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[2, 1, 3, 255]);

        let stream = webp_file(&webp_one_pixel(1, 2, 3, 0, 1));
        assert_eq!(decode_webp(&stream, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[2, 1, 3, 0]);
    }

    #[test]
    fn webp_code_length_repeat_uses_the_previous_non_zero_length() {
        let _guard = scratch_guard();
        let mut writer = Vp8lWriter::new();
        writer.put(0, 1); // normal prefix code
        writer.put(5, 4); // nine code-length codes
        // Order is 17, 18, 0, 1, 2, 3, 4, 5, 16.
        for length in [0u32, 0, 1, 0, 0, 0, 0, 2, 2] {
            writer.put(length, 3);
        }
        writer.put(0, 1); // max_symbol is the whole alphabet
        // Symbol 0 has code 0; symbols 5 and 16 have codes 10 and 11.
        writer.put(1, 1);
        writer.put(0, 1); // literal length five
        writer.put(0, 1); // literal length zero
        writer.put(1, 1);
        writer.put(1, 1); // repeat the previous non-zero length
        writer.put(0, 2); // three times
        writer.put(0, 1); // literal length zero

        let mut bits = WebpBits::new(&writer.bytes);
        let mut lengths = [0u8; 6];
        let mut symbols = [0u16; 16];
        let mut used = 0usize;
        let table = webp_read_huffman(&mut bits, &mut lengths, &mut symbols, &mut used).unwrap();
        // The zero between the two length-five runs must not become the source
        // of the repeat: code 16 repeats the last non-zero length.
        assert_eq!(lengths, [5, 0, 5, 5, 5, 0]);
        assert_eq!(table.single, WEBP_NO_SYMBOL);
    }

    #[test]
    fn webp_lossless_decodes_two_by_two_literals() {
        let _guard = scratch_guard();
        let mut body = Vp8lWriter::new();
        put_stream_start(&mut body);
        put_group(&mut body, &[10, 20], &[40, 50], &[60, 70], &[255], &[0]);
        // Symbol order fixes the codes: the lower symbol of each tree is zero.
        for (green, red, blue) in [(10u16, 40u16, 60u16), (20, 50, 70), (10, 40, 60), (20, 50, 70)]
        {
            body.put(u32::from(green == 20), 1);
            body.put(u32::from(red == 50), 1);
            body.put(u32::from(blue == 70), 1);
        }
        let stream = webp_file(&vp8l_stream(2, 2, 0, body));
        let mut out = [0u8; 16];
        assert_eq!(decode_webp(&stream, &mut out), Ok((2, 2, 16)));
        assert_eq!(&out[0..4], &[40, 10, 60, 255]);
        assert_eq!(&out[4..8], &[50, 20, 70, 255]);
        assert_eq!(&out[8..12], &[40, 10, 60, 255]);
        assert_eq!(&out[12..16], &[50, 20, 70, 255]);
    }

    #[test]
    fn webp_lossless_preserves_the_alpha_channel() {
        let _guard = scratch_guard();
        let mut body = Vp8lWriter::new();
        put_stream_start(&mut body);
        put_group(&mut body, &[9], &[1], &[2], &[255, 64], &[0]);
        // 64 sorts before 255, so its code is zero.
        body.put(0, 1);
        body.put(1, 1);
        let stream = webp_file(&vp8l_stream(2, 1, 1, body));
        let mut out = [0u8; 8];
        assert_eq!(decode_webp(&stream, &mut out), Ok((2, 1, 8)));
        assert_eq!(&out[0..4], &[1, 9, 2, 64]);
        assert_eq!(&out[4..8], &[1, 9, 2, 255]);
    }

    #[test]
    fn webp_lossless_follows_a_backward_reference() {
        let _guard = scratch_guard();
        let mut body = Vp8lWriter::new();
        put_stream_start(&mut body);
        put_two_symbol_code(&mut body, WEBP_GREEN_CODES, [5, 258]);
        put_simple_code(&mut body, &[0x11]);
        put_simple_code(&mut body, &[0x22]);
        put_simple_code(&mut body, &[0xFF]);
        put_simple_code(&mut body, &[1]);
        // A literal pixel, then a length-three copy at distance one.
        body.put(0, 1);
        body.put(1, 1);
        let stream = webp_file(&vp8l_stream(2, 2, 0, body));
        let mut out = [0u8; 16];
        assert_eq!(decode_webp(&stream, &mut out), Ok((2, 2, 16)));
        for pixel in out.chunks_exact(4) {
            assert_eq!(pixel, &[0x11, 5, 0x22, 0xFF]);
        }
    }

    #[test]
    fn webp_lossless_applies_subtract_green_and_predictor() {
        let _guard = scratch_guard();
        let mut body = Vp8lWriter::new();
        body.put(1, 1); // a transform follows
        body.put(2, 2); // subtract green
        body.put(1, 1); // a transform follows
        body.put(0, 2); // predictor
        body.put(0, 3); // 4x4 blocks
        body.put(0, 1); // the predictor image has no colour cache
        put_group(&mut body, &[7], &[0], &[0], &[255], &[0]);
        body.put(0, 1); // end of transforms
        body.put(0, 1); // no colour cache
        body.put(0, 1); // one prefix-code group
        put_group(&mut body, &[0, 4], &[0, 10], &[0, 30], &[0], &[0]);
        // Residuals below 0xff000000: the first three pixels share one colour,
        // and the fourth adds four to green so the average is exercised.
        for (green, red, blue) in [(1u32, 1u32, 1u32), (0, 0, 0), (0, 0, 0), (1, 0, 0)] {
            body.put(green, 1);
            body.put(red, 1);
            body.put(blue, 1);
        }
        let stream = webp_file(&vp8l_stream(2, 2, 1, body));
        let mut out = [0u8; 16];
        assert_eq!(decode_webp(&stream, &mut out), Ok((2, 2, 16)));
        assert_eq!(&out[0..4], &[14, 4, 34, 255]);
        assert_eq!(&out[4..8], &[14, 4, 34, 255]);
        assert_eq!(&out[8..12], &[14, 4, 34, 255]);
        assert_eq!(&out[12..16], &[18, 8, 38, 255]);
    }

    #[test]
    fn webp_lossless_reads_the_colour_cache() {
        let _guard = scratch_guard();
        let pixel = (0xFFu32 << 24) | (1 << 16) | (5 << 8) | 2;
        let slot = (pixel.wrapping_mul(0x1e35_a7bd) >> 31) as u16;
        let mut body = Vp8lWriter::new();
        body.put(0, 1); // no transform
        body.put(1, 1);
        body.put(1, 4); // two cache entries
        body.put(0, 1); // one prefix-code group
        put_two_symbol_code(
            &mut body,
            WEBP_GREEN_CODES + 2,
            [5, WEBP_GREEN_CODES as u16 + slot],
        );
        put_simple_code(&mut body, &[1]);
        put_simple_code(&mut body, &[2]);
        put_simple_code(&mut body, &[0xFF]);
        put_simple_code(&mut body, &[0]);
        body.put(0, 1); // literal green 5
        body.put(1, 1); // the cache entry it filled
        let stream = webp_file(&vp8l_stream(2, 1, 0, body));
        let mut out = [0u8; 8];
        assert_eq!(decode_webp(&stream, &mut out), Ok((2, 1, 8)));
        assert_eq!(&out[0..4], &[1, 5, 2, 0xFF]);
        assert_eq!(&out[4..8], &[1, 5, 2, 0xFF]);
    }

    #[test]
    fn webp_lossless_reads_the_extended_container() {
        let _guard = scratch_guard();
        let image = webp_one_pixel(0x33, 0x44, 0x55, 0xFF, 0);
        let mut vp8x = Vec::new();
        vp8x.extend_from_slice(&[0, 0, 0, 0]);
        vp8x.extend_from_slice(&[0, 0, 0]);
        vp8x.extend_from_slice(&[0, 0, 0]);
        let stream = riff_file(&[
            (b"VP8X", &vp8x),
            (b"ICCP", b"profile"),
            (b"VP8L", &image),
            (b"EXIF", b"exif"),
        ]);
        assert_eq!(decode_webp_header(&stream), Ok((1, 1)));
        let mut out = [0u8; 4];
        assert_eq!(decode_webp(&stream, &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[0x44, 0x33, 0x55, 0xFF]);
    }

    #[test]
    fn webp_lossless_refuses_truncated_and_lossy_streams() {
        let _guard = scratch_guard();
        let mut out = [0u8; 16];
        let image = webp_one_pixel(0x40, 0x20, 0x10, 0xFF, 0);
        let stream = webp_file(&image);
        assert!(decode_webp(&[], &mut out).is_err());
        assert!(decode_webp(&stream[..11], &mut out).is_err());
        let mut truncated = stream.clone();
        truncated.truncate(stream.len() - 2);
        assert!(decode_webp(&truncated, &mut out).is_err());

        let mut header_only = image.clone();
        header_only.truncate(5);
        assert!(decode_webp(&webp_file(&header_only), &mut out).is_err());

        let empty = riff_file(&[(b"VP8L", b"")]);
        assert!(decode_webp(&empty, &mut out).is_err());

        let lossy = riff_file(&[(b"VP8 ", &[0u8; 16])]);
        assert!(decode_webp(&lossy, &mut out).is_err());
        assert!(decode_webp_header(&lossy).is_err());

        let mut bad_version = stream.clone();
        bad_version[4] |= 0x20;
        assert!(decode_webp(&bad_version, &mut out).is_err());
    }

    #[test]
    fn webp_lossless_refuses_a_small_output_buffer() {
        let _guard = scratch_guard();
        let mut body = Vp8lWriter::new();
        put_stream_start(&mut body);
        put_group(&mut body, &[1], &[1], &[1], &[255], &[0]);
        // A 2x2 image needs sixteen bytes, all four pixels being literals.
        let stream = webp_file(&vp8l_stream(2, 2, 0, body));
        let mut out = [0u8; 15];
        assert!(decode_webp(&stream, &mut out).is_err());
    }

    #[test]
    fn decode_image_routes_webp() {
        let _guard = scratch_guard();
        let stream = webp_file(&webp_one_pixel(0x40, 0x20, 0x10, 0xFF, 0));
        let mut out = [0u8; 4];
        assert_eq!(decode_image(&stream, b"mystery", &mut out), Ok((1, 1, 4)));
        assert_eq!(&out, &[0x20, 0x40, 0x10, 0xFF]);
        assert_eq!(
            decode_image(&stream, b"picture.WEBP", &mut out),
            Ok((1, 1, 4))
        );
        assert!(decode_image(b"not a webp", b"picture.webp", &mut out).is_err());
    }

    // -- ICO ---------------------------------------------------------------

    /// A device-independent bitmap as an `.ico` entry stores one: no file
    /// header, a height that counts the pixels and the mask beneath them, and
    /// rows written bottom-up. `rows` is given top-down, as a reader thinks
    /// of a picture.
    fn ico_dib(rows: &[&[[u8; 4]]], mask: &[u8]) -> Vec<u8> {
        let width = rows[0].len() as u32;
        let height = rows.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(width as i32).to_le_bytes());
        out.extend_from_slice(&((height * 2) as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 16]);
        for row in rows.iter().rev() {
            for pixel in row.iter() {
                out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        out.extend_from_slice(mask);
        out
    }

    /// Wraps payloads in an `.ico` header and directory, laying them out at
    /// the offsets the directory names.
    fn ico_container(entries: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0u8, 0, 1, 0];
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut offset = out.len() + entries.len() * 16;
        for (width, height, payload) in entries {
            out.push(if *width == 256 { 0 } else { *width as u8 });
            out.push(if *height == 256 { 0 } else { *height as u8 });
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            offset += payload.len();
        }
        for (_, _, payload) in entries {
            out.extend_from_slice(payload);
        }
        out
    }

    #[test]
    fn an_icon_container_is_read_from_its_largest_picture() {
        let small = ico_dib(&[&[[10, 20, 30, 255]]], &[]);
        let large = ico_dib(
            &[
                &[[10, 20, 30, 255], [40, 50, 60, 255]],
                &[[70, 80, 90, 255], [100, 110, 120, 255]],
            ],
            &[],
        );
        // The small entry is listed first: the directory is not ordered by
        // size, and a reader that took the first entry would blur.
        let file = ico_container(&[(1, 1, small), (2, 2, large)]);
        assert_eq!(decode_ico_header(&file), Ok((2, 2)));

        let mut out = [0u8; 2 * 2 * 4];
        assert_eq!(decode_ico(&file, &mut out), Ok((2, 2, 16)));
        assert_eq!(
            out,
            [
                10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 255
            ]
        );
    }

    #[test]
    fn an_icon_that_never_filled_its_alpha_takes_its_shape_from_the_mask() {
        // Every alpha byte is zero, so the fourth byte says nothing, and the
        // one-bit mask under the pixels is the only account of the shape. It
        // is stored bottom-up with them, so the top row is the second one.
        let pixels: Vec<[u8; 4]> = vec![[1, 2, 3, 0]; 4];
        let dib = ico_dib(&[&pixels[..2], &pixels[2..]], &[0x00, 0, 0, 0, 0x80, 0, 0, 0]);
        let file = ico_container(&[(2, 2, dib)]);
        let mut out = [0u8; 2 * 2 * 4];
        assert_eq!(decode_ico(&file, &mut out), Ok((2, 2, 16)));
        assert_eq!(out[3], 0, "the masked pixel is transparent");
        assert_eq!(out[7], 255);
        assert_eq!(out[11], 255);
        assert_eq!(&out[..3], &[1, 2, 3], "the colour survives");
    }

    #[test]
    fn a_png_inside_an_icon_goes_to_the_png_reader() {
        let _guard = scratch_guard();
        let png = build_png(1, 1, 6, 0, None, None, &[0, 0x20, 0x40, 0x10, 0xFF]);
        let file = ico_container(&[(1, 1, png)]);
        let mut out = [0u8; 4];
        assert_eq!(decode_ico(&file, &mut out), Ok((1, 1, 4)));
        assert_eq!(out, [0x20, 0x40, 0x10, 0xFF]);
    }

    #[test]
    fn an_icon_directory_that_does_not_hold_a_picture_is_refused() {
        // A file that starts with the signature and then describes a picture
        // whose bytes are not in the file.
        let mut file = vec![0u8, 0, 1, 0, 1, 0];
        file.extend_from_slice(&[2, 2, 0, 0, 1, 0, 32, 0]);
        file.extend_from_slice(&200u32.to_le_bytes());
        file.extend_from_slice(&999_999u32.to_le_bytes());
        assert_eq!(decode_ico_header(&file), Err(()));
        assert!(decode_ico(&file, &mut [0u8; 64]).is_err());

        // Four bytes that rhyme with the signature are not an icon.
        assert!(decode_image(&[0, 0, 1, 0, 0, 0, 0, 0], b"mystery", &mut [0u8; 4]).is_err());
    }

    #[test]
    fn decode_image_routes_an_icon_by_signature_and_by_name() {
        let dib = ico_dib(&[&[[7, 8, 9, 255]]], &[]);
        let file = ico_container(&[(1, 1, dib)]);
        let mut out = [0u8; 4];
        assert_eq!(decode_image(&file, b"mystery", &mut out), Ok((1, 1, 4)));
        assert_eq!(out, [7, 8, 9, 255]);
        assert_eq!(decode_image(&file, b"favicon.ICO", &mut out), Ok((1, 1, 4)));
    }
}

