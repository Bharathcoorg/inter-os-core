#![no_std]

pub use interos_std as std;

pub const IMAGE_SUFFIXES: [&[u8]; 9] = [
    b".jpg", b".jpeg", b".png", b".bmp", b".gif", b".webp", b".qoi", b".ppm", b".raw",
];

pub const VIDEO_SUFFIXES: [&[u8]; 5] = [
    b".mp4", b".webm", b".mkv", b".avi", b".mov",
];

/// Whether an entry name carries a recognised image extension (case-insensitive).
pub fn has_image_suffix(name: &[u8]) -> bool {
    IMAGE_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Whether an entry name carries a recognised video extension (case-insensitive).
pub fn has_video_suffix(name: &[u8]) -> bool {
    VIDEO_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Whether an entry name carries any media extension (image or video).
pub fn is_media_file(name: &[u8]) -> bool {
    has_image_suffix(name) || has_video_suffix(name)
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

/// Width of a decoded thumbnail, in pixels.
pub const THUMB_W: usize = 54;

/// Height of a decoded thumbnail, in pixels.
pub const THUMB_H: usize = 42;

/// Number of bytes a decoded thumbnail occupies (RGB triples).
pub const THUMB_BYTES: usize = THUMB_W * THUMB_H * 3;

/// A decoded thumbnail of a stored image.
///
/// WHY a thumbnail exists at all: the grid used to paint each stored file
/// with a gradient whose colours were hashed from the file name. That is a
/// picture of nothing -- it looks like a preview of the photograph and is
/// not one. A thumbnail here is either the real image, decoded from the
/// file, or it is absent and the tile says so.
#[derive(Clone, Copy)]
pub struct Thumbnail {
    /// RGB triples, row-major.
    pub rgb: [u8; THUMB_BYTES],
    /// Whether the thumbnail holds a decoded image.
    pub valid: bool,
}

impl Thumbnail {
    /// The empty thumbnail -- no image was decoded.
    pub const EMPTY: Self = Self {
        rgb: [0u8; THUMB_BYTES],
        valid: false,
    };

    /// Scales a decoded RGBA image down into a thumbnail.
    ///
    /// Returns `None` when the source is empty. A source that is smaller
    /// than the thumbnail is sampled by nearest neighbour, which is enough
    /// for a preview and never invents a pixel that was not in the file.
    pub fn from_rgba(rgba: &[u8], src_w: u32, src_h: u32) -> Option<Self> {
        if src_w == 0 || src_h == 0 {
            return None;
        }
        let mut thumb = Self {
            rgb: [0u8; THUMB_BYTES],
            valid: true,
        };
        for ty in 0..THUMB_H {
            // Nearest-neighbour sample: pick the source row that this
            // thumbnail row maps to.
            let sy = (ty as u64 * src_h as u64 / THUMB_H as u64) as usize;
            let sy = if sy >= src_h as usize { src_h as usize - 1 } else { sy };
            for tx in 0..THUMB_W {
                let sx = (tx as u64 * src_w as u64 / THUMB_W as u64) as usize;
                let sx = if sx >= src_w as usize { src_w as usize - 1 } else { sx };
                let src = (sy * src_w as usize + sx) * 4;
                if src + 3 >= rgba.len() {
                    continue;
                }
                let dst = (ty * THUMB_W + tx) * 3;
                thumb.rgb[dst] = rgba[src];
                thumb.rgb[dst + 1] = rgba[src + 1];
                thumb.rgb[dst + 2] = rgba[src + 2];
            }
        }
        Some(thumb)
    }

    /// The colour of the thumbnail at a point inside the tile.
    ///
    /// Returns `None` when the thumbnail holds no image, so the caller can
    /// draw a placeholder rather than a colour.
    pub fn sample(&self, tx: usize, ty: usize) -> Option<u32> {
        if !self.valid {
            return None;
        }
        if tx >= THUMB_W || ty >= THUMB_H {
            return None;
        }
        let i = (ty * THUMB_W + tx) * 3;
        Some(
            0xFF000000
                | ((self.rgb[i] as u32) << 16)
                | ((self.rgb[i + 1] as u32) << 8)
                | (self.rgb[i + 2] as u32),
        )
    }
}

/// One media item the gallery can show or play.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Wallpaper(usize),
    Photo(usize),
    StorageItem(usize),
}

/// Which set the filter row selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Wallpapers,
    Photos,
    Videos,
}

impl Filter {
    pub fn from_index(index: usize) -> Filter {
        match index {
            1 => Filter::Wallpapers,
            2 => Filter::Photos,
            3 => Filter::Videos,
            _ => Filter::All,
        }
    }

    pub fn shows_wallpapers(self) -> bool {
        matches!(self, Filter::All | Filter::Wallpapers)
    }

    pub fn shows_ledger(self) -> bool {
        matches!(self, Filter::All | Filter::Photos)
    }

    pub fn shows_videos(self) -> bool {
        matches!(self, Filter::All | Filter::Videos)
    }
}

pub const MAX_STORAGE_ITEMS: usize = 16;

/// A media file discovered on guest storage (/storage/photos, /storage/pictures, etc.)
#[derive(Clone, Copy, Debug)]
pub struct StorageMediaItem {
    pub name: [u8; 48],
    pub name_len: usize,
    pub path: [u8; 64],
    pub path_len: usize,
    pub size: u64,
    pub is_video: bool,
    pub is_favourite: bool,
    pub width: u32,
    pub height: u32,
    pub primary_color: u32,
    pub secondary_color: u32,
}

impl StorageMediaItem {
    pub const EMPTY: Self = Self {
        name: [0u8; 48],
        name_len: 0,
        path: [0u8; 64],
        path_len: 0,
        size: 0,
        is_video: false,
        is_favourite: false,
        width: 720,
        height: 1440,
        primary_color: 0xFF1E293B,
        secondary_color: 0xFF0F172A,
    };
}

/// Supported mobile video codecs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoCodec {
    /// The container was not parsed, so the codec is not known.
    Undetermined,
}

impl VideoCodec {
    /// A description of what is actually known about the codec.
    ///
    /// WHY this says almost nothing: a file extension names a *container*,
    /// not a codec. An `.mp4` may carry H.264, HEVC or AV1; a `.webm` may
    /// carry VP8, VP9 or AV1; an `.mkv` may carry anything at all. The old
    /// code mapped extensions to codecs and then printed claims such as
    /// "H.264 / AVC 1080p", "VP9 - 60 FPS" and "AV1 - HDR10" -- a codec, a
    /// resolution, a frame rate and a colour profile, none of which had
    /// been read from a file. Until the container is parsed, the codec is
    /// undetermined, and that is what the screen says.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undetermined => "Codec undetermined (container not parsed)",
        }
    }
}

/// Modern media gallery tabs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GalleryTab {
    Photos = 0,
    Videos = 1,
    Albums = 2,
    Screenshots = 3,
}

impl GalleryTab {
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => GalleryTab::Videos,
            2 => GalleryTab::Albums,
            3 => GalleryTab::Screenshots,
            _ => GalleryTab::Photos,
        }
    }
}

/// Video player runtime state with playback controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VideoPlayerState {
    pub is_playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub is_fullscreen: bool,
    pub is_fill: bool,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
}

impl VideoPlayerState {
    /// Creates a player state for a file whose duration is not known.
    ///
    /// WHY the duration is not an argument: the gallery used to open every
    /// video with a hardcoded 30-second duration, which the scrubber, the
    /// elapsed clock and the progress bar then displayed as though it had
    /// been measured. A container's duration is only known once the
    /// container is parsed, so until then the duration is zero and the
    /// screen shows that it is unknown.
    pub const fn new_undetermined() -> Self {
        Self {
            is_playing: false,
            position_ms: 0,
            duration_ms: 0,
            is_fullscreen: false,
            is_fill: false,
            codec: VideoCodec::Undetermined,
            width: 0,
            height: 0,
        }
    }

    /// Whether the media's duration is known.
    pub const fn has_known_duration(&self) -> bool {
        self.duration_ms > 0
    }

    pub fn toggle_play(&mut self) {
        self.is_playing = !self.is_playing;
    }

    pub fn toggle_aspect(&mut self) {
        self.is_fill = !self.is_fill;
    }

    pub fn advance_ticks(&mut self, ticks: u32) {
        if self.is_playing && self.duration_ms > 0 {
            let next = self.position_ms.saturating_add(ticks.saturating_mul(16));
            if next >= self.duration_ms {
                self.position_ms = 0; // loop
            } else {
                self.position_ms = next;
            }
        }
    }

    pub fn seek_to_ratio(&mut self, ratio_milli: u32) {
        let clamped = ratio_milli.min(1000);
        self.position_ms = (self.duration_ms as u64 * clamped as u64 / 1000) as u32;
    }
}

/// Resolves the media item at a flat grid index.
pub fn media_at(index: usize, wallpaper_count: usize, photo_count: usize) -> Option<Media> {
    media_at_with_storage(index, wallpaper_count, photo_count, 0)
}

/// Resolves the media item at a flat grid index including storage items.
pub fn media_at_with_storage(
    index: usize,
    wallpaper_count: usize,
    photo_count: usize,
    storage_count: usize,
) -> Option<Media> {
    if index < wallpaper_count {
        return Some(Media::Wallpaper(index));
    }
    let photo = index - wallpaper_count;
    if photo < photo_count {
        return Some(Media::Photo(photo));
    }
    let storage = photo - photo_count;
    if storage < storage_count {
        return Some(Media::StorageItem(storage));
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
        assert!(has_image_suffix(b"screenshot.ppm"));
        assert!(has_image_suffix(b"frame.raw"));

        assert!(!has_image_suffix(b"document.txt"));
        assert!(!has_image_suffix(b"binary.exe"));
        assert!(!has_image_suffix(b"manifest.json"));
        assert!(!has_image_suffix(b".png")); // suffix without base name
        assert!(!has_image_suffix(b""));
    }

    #[test]
    fn test_video_suffix_detection() {
        assert!(has_video_suffix(b"clip.mp4"));
        assert!(has_video_suffix(b"MOVIE.WEBM"));
        assert!(has_video_suffix(b"teaser.mkv"));
        assert!(has_video_suffix(b"stream.mov"));
        assert!(has_video_suffix(b"old.avi"));

        assert!(!has_video_suffix(b"photo.jpg"));
        assert!(!has_video_suffix(b"audio.mp3"));
        assert!(!has_video_suffix(b""));
    }

    #[test]
    fn test_is_media_file() {
        assert!(is_media_file(b"pic.jpg"));
        assert!(is_media_file(b"vid.mp4"));
        assert!(!is_media_file(b"data.bin"));
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
        assert!(f_all.shows_videos());

        let f_wall = Filter::from_index(1);
        assert!(f_wall.shows_wallpapers());
        assert!(!f_wall.shows_ledger());
        assert!(!f_wall.shows_videos());

        let f_photo = Filter::from_index(2);
        assert!(!f_photo.shows_wallpapers());
        assert!(f_photo.shows_ledger());
        assert!(!f_photo.shows_videos());

        let f_video = Filter::from_index(3);
        assert!(!f_video.shows_wallpapers());
        assert!(!f_video.shows_ledger());
        assert!(f_video.shows_videos());

        assert_eq!(media_at(0, 3, 2), Some(Media::Wallpaper(0)));
        assert_eq!(media_at(2, 3, 2), Some(Media::Wallpaper(2)));
        assert_eq!(media_at(3, 3, 2), Some(Media::Photo(0)));
        assert_eq!(media_at(4, 3, 2), Some(Media::Photo(1)));
        assert_eq!(media_at(5, 3, 2), None);

        assert_eq!(media_at_with_storage(5, 3, 2, 2), Some(Media::StorageItem(0)));
        assert_eq!(media_at_with_storage(6, 3, 2, 2), Some(Media::StorageItem(1)));
        assert_eq!(media_at_with_storage(7, 3, 2, 2), None);
    }

    #[test]
    fn test_storage_media_and_video_state() {
        let mut item = StorageMediaItem::EMPTY;
        assert_eq!(item.name_len, 0);
        assert!(!item.is_video);
        item.is_video = true;
        item.is_favourite = true;
        assert!(item.is_video);
        assert!(item.is_favourite);

        let mut player = VideoPlayerState::new_undetermined();
        // WHY the duration starts unknown: the gallery cannot parse a
        // container, so it must not claim a length it never read.
        assert!(!player.is_playing);
        assert_eq!(player.position_ms, 0);
        assert_eq!(player.duration_ms, 0);
        assert!(!player.has_known_duration());
        assert_eq!(player.codec.as_str(), "Codec undetermined (container not parsed)");

        // A player with no known duration never advances its position.
        player.advance_ticks(10);
        assert_eq!(player.position_ms, 0);

        // A player with a known duration advances and seeks.
        player.duration_ms = 60_000;
        player.is_playing = true;
        assert!(player.has_known_duration());
        player.advance_ticks(10);
        assert_eq!(player.position_ms, 160);

        player.seek_to_ratio(500); // 50%
        assert_eq!(player.position_ms, 30_000);

        player.toggle_play();
        assert!(!player.is_playing);
        player.advance_ticks(10);
        assert_eq!(player.position_ms, 30_000); // does not advance when paused
    }

    #[test]
    fn test_a_thumbnail_holds_only_decoded_pixels() {
        // WHY this matters: the grid used to fill each stored file with a
        // gradient hashed from its name. A thumbnail is either the real
        // image or it is absent.
        let empty = Thumbnail::EMPTY;
        assert!(!empty.valid);
        assert_eq!(empty.sample(0, 0), None);

        // A two-by-two red image, scaled into the thumbnail.
        let mut rgba = [0u8; 4 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 255;
            px[1] = 0;
            px[2] = 0;
            px[3] = 255;
        }
        let thumb = Thumbnail::from_rgba(&rgba, 2, 2).expect("thumbnail");
        assert!(thumb.valid);
        let colour = thumb.sample(0, 0).expect("a pixel");
        assert_eq!(colour & 0x00FF_FFFF, 0x00FF_0000);

        // A zero-sized source is not an image.
        assert!(Thumbnail::from_rgba(&rgba, 0, 2).is_none());
    }

    #[test]
    fn test_ppm_encode_decode() {
        let rgb_data = [
            255, 0, 0,   // Red
            0, 255, 0,   // Green
            0, 0, 255,   // Blue
            255, 255, 0, // Yellow
        ];
        let mut ppm_buf = [0u8; 64];
        let n = encode_ppm(2, 2, &rgb_data, false, &mut ppm_buf).expect("ppm encode");
        assert!(n > 12);

        let (w, h, offset) = decode_ppm_header(&ppm_buf[..n]).expect("ppm header");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert!(offset > 0);

        let mut out_rgba = [0u8; 16];
        let decoded_len = decode_ppm(&ppm_buf[..n], &mut out_rgba).expect("ppm decode");
        assert_eq!(decoded_len, 16);
        assert_eq!(&out_rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out_rgba[4..8], &[0, 255, 0, 255]);
        assert_eq!(&out_rgba[8..12], &[0, 0, 255, 255]);
        assert_eq!(&out_rgba[12..16], &[255, 255, 0, 255]);
    }

    #[test]
    fn test_bmp_decode() {
        // Minimal 2x2 24bpp BMP
        // File header: 14 bytes
        // DIB header: 40 bytes
        // Pixel data: 2 rows of 2 pixels (6 bytes + 2 bytes padding = 8 bytes per row -> 16 bytes)
        // Total = 70 bytes
        let mut bmp = [0u8; 70];
        // File header
        bmp[0] = b'B';
        bmp[1] = b'M';
        bmp[2..6].copy_from_slice(&(70u32.to_le_bytes()));
        bmp[10..14].copy_from_slice(&(54u32.to_le_bytes())); // data offset
        // DIB header
        bmp[14..18].copy_from_slice(&(40u32.to_le_bytes())); // header size
        bmp[18..22].copy_from_slice(&(2i32.to_le_bytes()));  // width = 2
        bmp[22..26].copy_from_slice(&(2i32.to_le_bytes()));  // height = 2 (bottom-up)
        bmp[26..28].copy_from_slice(&(1u16.to_le_bytes()));  // planes = 1
        bmp[28..30].copy_from_slice(&(24u16.to_le_bytes())); // bpp = 24
        bmp[30..34].copy_from_slice(&(0u32.to_le_bytes()));  // BI_RGB = 0

        // Row 0 (bottom row): Pixel (0, 1): Blue (255, 0, 0 in BGR), Pixel (1, 1): White (255, 255, 255)
        let row0_start = 54;
        bmp[row0_start..row0_start + 3].copy_from_slice(&[255, 0, 0]); // BGR Blue
        bmp[row0_start + 3..row0_start + 6].copy_from_slice(&[255, 255, 255]); // BGR White
        // 2 bytes padding
        bmp[row0_start + 6] = 0;
        bmp[row0_start + 7] = 0;

        // Row 1 (top row): Pixel (0, 0): Red (0, 0, 255 in BGR), Pixel (1, 0): Green (0, 255, 0 in BGR)
        let row1_start = 62;
        bmp[row1_start..row1_start + 3].copy_from_slice(&[0, 0, 255]); // BGR Red
        bmp[row1_start + 3..row1_start + 6].copy_from_slice(&[0, 255, 0]); // BGR Green
        bmp[row1_start + 6] = 0;
        bmp[row1_start + 7] = 0;

        let (w, h, bpp, offset) = decode_bmp_header(&bmp).expect("bmp header");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(bpp, 24);
        assert_eq!(offset, 54);

        let mut out_rgba = [0u8; 16];
        let n = decode_bmp(&bmp, &mut out_rgba).expect("bmp decode");
        assert_eq!(n, 16);
        // Top row first in decoded RGBA:
        // (0,0) Red
        assert_eq!(&out_rgba[0..4], &[255, 0, 0, 255]);
        // (1,0) Green
        assert_eq!(&out_rgba[4..8], &[0, 255, 0, 255]);
        // (0,1) Blue
        assert_eq!(&out_rgba[8..12], &[0, 0, 255, 255]);
        // (1,1) White
        assert_eq!(&out_rgba[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn test_raw_decode() {
        let raw = [10, 20, 30, 40, 50, 60];
        let mut rgba = [0u8; 8];
        let n = decode_raw(&raw, 2, 1, 24, &mut rgba).expect("raw decode");
        assert_eq!(n, 8);
        assert_eq!(&rgba[0..4], &[10, 20, 30, 255]);
        assert_eq!(&rgba[4..8], &[40, 50, 60, 255]);
    }

    #[test]
    fn test_qoi_suffix_and_decode() {
        assert!(has_image_suffix(b"photo.qoi"));
        assert!(has_image_suffix(b"CAPTURE.QOI"));

        // Minimal valid 1x1 red QOI image (RGBA: 255, 0, 0, 255)
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

