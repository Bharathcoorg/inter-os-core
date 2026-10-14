#![no_std]

pub use interos_std as std;

pub const IMAGE_SUFFIXES: [&[u8]; 7] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp", b".qoi",
];

/// Whether an entry name carries a recognised image extension (case-insensitive).
pub fn has_image_suffix(name: &[u8]) -> bool {
    IMAGE_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Counts total entries and image files in a VFS `fs_list` payload formatted
/// as newline-separated `name|size|type` lines.
pub fn count_listing(payload: &[u8]) -> (usize, usize) {
    let mut entries = 0usize;
    let mut images = 0usize;
    for line in payload.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        entries += 1;
        let end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
        if has_image_suffix(&line[..end]) {
            images += 1;
        }
    }
    (entries, images)
}

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

/// One image the gallery can show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Wallpaper(usize),
    Photo(usize),
}

/// Which set the filter row selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Wallpapers,
    Photos,
}

impl Filter {
    pub fn from_index(index: usize) -> Filter {
        match index {
            1 => Filter::Wallpapers,
            2 => Filter::Photos,
            _ => Filter::All,
        }
    }

    pub fn shows_wallpapers(self) -> bool {
        matches!(self, Filter::All | Filter::Wallpapers)
    }

    pub fn shows_ledger(self) -> bool {
        matches!(self, Filter::All | Filter::Photos)
    }
}

/// Resolves the media item at a flat grid index.
pub fn media_at(index: usize, wallpaper_count: usize, photo_count: usize) -> Option<Media> {
    if index < wallpaper_count {
        return Some(Media::Wallpaper(index));
    }
    let photo = index - wallpaper_count;
    if photo < photo_count {
        return Some(Media::Photo(photo));
    }
    None
}

/// Hashes an RGBA color tuple into the 64-entry running color cache.
#[inline(always)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_suffix_detection() {
        assert!(has_image_suffix(b"photo.jpg"));
        assert!(has_image_suffix(b"IMAGE.PNG"));
        assert!(has_image_suffix(b"graphic.bmp"));
        assert!(has_image_suffix(b"anim.gif"));
        assert!(has_image_suffix(b"shot.webp"));
        assert!(has_image_suffix(b"picture.jpeg"));

        assert!(!has_image_suffix(b"document.txt"));
        assert!(!has_image_suffix(b"binary.exe"));
        assert!(!has_image_suffix(b"manifest.json"));
        assert!(!has_image_suffix(b".png")); // suffix without base name
        assert!(!has_image_suffix(b""));
    }

    #[test]
    fn test_count_listing() {
        let payload = b"wallpaper.jpg|10240|file\nreadme.txt|512|file\npic.PNG|2048|file\nempty_dir|0|dir\n";
        let (entries, images) = count_listing(payload);
        assert_eq!(entries, 4);
        assert_eq!(images, 2);
    }

    #[test]
    fn test_append_u64_formatting() {
        let mut buf = [0u8; 32];
        let n = append_u64(&mut buf, 0, 0);
        assert_eq!(&buf[..n], b"0");

        let n2 = append_u64(&mut buf, 0, 1440);
        assert_eq!(&buf[..n2], b"1440");

        let n3 = append_u64(&mut buf, 0, 9876543210);
        assert_eq!(&buf[..n3], b"9876543210");
    }

    #[test]
    fn test_filter_and_media_at() {
        let f_all = Filter::from_index(0);
        assert!(f_all.shows_wallpapers());
        assert!(f_all.shows_ledger());

        let f_wall = Filter::from_index(1);
        assert!(f_wall.shows_wallpapers());
        assert!(!f_wall.shows_ledger());

        let f_photo = Filter::from_index(2);
        assert!(!f_photo.shows_wallpapers());
        assert!(f_photo.shows_ledger());

        assert_eq!(media_at(0, 3, 2), Some(Media::Wallpaper(0)));
        assert_eq!(media_at(2, 3, 2), Some(Media::Wallpaper(2)));
        assert_eq!(media_at(3, 3, 2), Some(Media::Photo(0)));
        assert_eq!(media_at(4, 3, 2), Some(Media::Photo(1)));
        assert_eq!(media_at(5, 3, 2), None);
    }

    #[test]
    fn test_qoi_suffix_and_decode() {
        assert!(has_image_suffix(b"photo.qoi"));
        assert!(has_image_suffix(b"CAPTURE.QOI"));

        // Minimal valid 1x1 red QOI image (RGBA: 255, 0, 0, 255)
        // 14-byte header: 'q','o','i','f', 0,0,0,1, 0,0,0,1, 4, 0
        // Data: 0xFE, 255, 0, 0 (QOI_OP_RGB)
        // End: 0, 0, 0, 0, 0, 0, 0, 1
        let mut qoi_data = [
            b'q', b'o', b'i', b'f',
            0, 0, 0, 1, // width = 1
            0, 0, 0, 1, // height = 1
            4, 0,       // channels = 4, colorspace = 0
            0xFE, 255, 0, 0, // QOI_OP_RGB
            0, 0, 0, 0, 0, 0, 0, 1, // end marker
        ];

        let (w, h, ch, _) = decode_qoi_header(&qoi_data).expect("header decode");
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert_eq!(ch, 4);

        let mut out_rgba = [0u8; 4];
        let bytes_written = decode_qoi(&qoi_data, &mut out_rgba).expect("pixel decode");
        assert_eq!(bytes_written, 4);
        assert_eq!(out_rgba, [255, 0, 0, 255]);

        // Corrupt magic
        qoi_data[0] = b'x';
        assert_eq!(decode_qoi_header(&qoi_data), Err(()));
    }
}

