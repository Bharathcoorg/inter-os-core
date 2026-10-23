//! InterOS Native Video Player Engine (Ring-3)
//!
//! Provides the core media state machine, frame decoders (RAW, PPM P6, MJPEG),
//! codec detection, scrubber calculations, and platform truth contract enforcement.

#![no_std]

pub use interos_std as std;

pub mod theme;

/// Exact error string required by the Platform Truth Contract when H.264 decode is requested
/// without hardware acceleration.
pub const NO_HW_H264_MSG: &str = "No hardware H.264 decoder bound. Software decode unavailable.";

/// Recognized video suffixes.
pub const VIDEO_SUFFIXES: [&[u8]; 7] = [
    b".ppm", b".raw", b".mjpg", b".mjpeg", b".mp4", b".h264", b".avi",
];

/// Checks if a file path/name carries a recognised video suffix (case-insensitive).
pub fn has_video_suffix(name: &[u8]) -> bool {
    VIDEO_SUFFIXES.iter().any(|suffix| {
        name.len() > suffix.len()
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Supported video codecs and container formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// Portable Pixmap binary RGB format (single or multi-frame).
    PpmP6,
    /// Uncompressed RAW pixel buffer (RGB 24-bit).
    RawRgb,
    /// Uncompressed RAW pixel buffer (RGBA 32-bit).
    RawRgba,
    /// Motion JPEG stream with SOI/SOF/EOI markers.
    Mjpeg,
    /// H.264 / AVC in MP4 or raw NAL stream (hardware decoder required).
    H264,
    /// Unknown or unsupported format.
    Unknown,
}

impl VideoCodec {
    pub const fn label(self) -> &'static str {
        match self {
            VideoCodec::PpmP6 => "PPM P6",
            VideoCodec::RawRgb => "RAW RGB24",
            VideoCodec::RawRgba => "RAW RGBA32",
            VideoCodec::Mjpeg => "MJPEG",
            VideoCodec::H264 => "H.264 / MP4",
            VideoCodec::Unknown => "UNKNOWN",
        }
    }
}

/// Detects the codec from a file name extension.
pub fn detect_codec_from_name(name: &[u8]) -> VideoCodec {
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".ppm") {
        VideoCodec::PpmP6
    } else if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".raw") {
        VideoCodec::RawRgb
    } else if (name.len() >= 5 && name[name.len() - 5..].eq_ignore_ascii_case(b".mjpg"))
        || (name.len() >= 6 && name[name.len() - 6..].eq_ignore_ascii_case(b".mjpeg"))
    {
        VideoCodec::Mjpeg
    } else if (name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".mp4"))
        || (name.len() >= 5 && name[name.len() - 5..].eq_ignore_ascii_case(b".h264"))
    {
        VideoCodec::H264
    } else {
        VideoCodec::Unknown
    }
}

/// Detects codec from data headers / magic bytes.
pub fn detect_codec_from_bytes(data: &[u8]) -> VideoCodec {
    if data.len() >= 2 && data[0] == b'P' && data[1] == b'6' {
        VideoCodec::PpmP6
    } else if data.len() >= 4 && &data[0..4] == b"RAWF" {
        if data.len() > 12 && data[12] == 4 {
            VideoCodec::RawRgba
        } else {
            VideoCodec::RawRgb
        }
    } else if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        VideoCodec::Mjpeg
    } else if (data.len() >= 8 && &data[4..8] == b"ftyp")
        || (data.len() >= 4 && data[0..3] == [0, 0, 0] && (data[3] == 1 || data[3] == 0))
    {
        VideoCodec::H264
    } else {
        VideoCodec::Unknown
    }
}

/// Playback transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped = 0,
    Playing = 1,
    Paused = 2,
}

impl PlaybackState {
    pub const fn label(self) -> &'static str {
        match self {
            PlaybackState::Stopped => "STOPPED",
            PlaybackState::Playing => "PLAYING",
            PlaybackState::Paused => "PAUSED",
        }
    }
}

/// Aspect ratio modes for video presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatio {
    /// 16:9 widescreen canvas standard.
    Fit16_9,
    /// 4:3 pillarboxed with black lateral bars.
    Aspect4_3,
    /// Stretched to completely fill the canvas bounds.
    Fill,
}

impl AspectRatio {
    pub const fn label(self) -> &'static str {
        match self {
            AspectRatio::Fit16_9 => "16:9",
            AspectRatio::Aspect4_3 => "4:3",
            AspectRatio::Fill => "FILL",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            AspectRatio::Fit16_9 => AspectRatio::Aspect4_3,
            AspectRatio::Aspect4_3 => AspectRatio::Fill,
            AspectRatio::Fill => AspectRatio::Fit16_9,
        }
    }
}

/// Playlist loop mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    LoopOne,
    LoopAll,
}

impl LoopMode {
    pub const fn label(self) -> &'static str {
        match self {
            LoopMode::None => "OFF",
            LoopMode::LoopOne => "ONE",
            LoopMode::LoopAll => "ALL",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            LoopMode::None => LoopMode::LoopOne,
            LoopMode::LoopOne => LoopMode::LoopAll,
            LoopMode::LoopAll => LoopMode::None,
        }
    }
}

/// Status of the active decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderStatus {
    Idle,
    Ready,
    HardwareUnavailable(&'static str),
    CorruptData,
    Error(&'static str),
}

/// Errors returned during video frame decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    NoHardwareDecoder,
    CorruptHeader,
    BufferTooSmall,
    UnsupportedDimension,
    EndOfStream,
    InvalidMagic,
}

// -----------------------------------------------------------------------------
// PPM P6 Frame Decoder
// -----------------------------------------------------------------------------

fn skip_ppm_whitespace(data: &[u8], mut pos: usize) -> usize {
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

fn parse_ppm_u32(data: &[u8], pos: &mut usize) -> Result<u32, DecodeError> {
    *pos = skip_ppm_whitespace(data, *pos);
    if *pos >= data.len() || !data[*pos].is_ascii_digit() {
        return Err(DecodeError::CorruptHeader);
    }
    let mut val: u32 = 0;
    while *pos < data.len() && data[*pos].is_ascii_digit() {
        val = val
            .checked_mul(10)
            .ok_or(DecodeError::CorruptHeader)?
            .checked_add((data[*pos] - b'0') as u32)
            .ok_or(DecodeError::CorruptHeader)?;
        *pos += 1;
    }
    Ok(val)
}

/// Reads PPM (P6 binary) header. Returns (width, height, data_start_offset).
pub fn parse_ppm_header(data: &[u8]) -> Result<(u32, u32, usize), DecodeError> {
    if data.len() < 3 || data[0] != b'P' || data[1] != b'6' {
        return Err(DecodeError::InvalidMagic);
    }
    if !data[2].is_ascii_whitespace() && data[2] != b'#' {
        return Err(DecodeError::CorruptHeader);
    }
    let mut pos = 2;
    let width = parse_ppm_u32(data, &mut pos)?;
    let height = parse_ppm_u32(data, &mut pos)?;
    let maxval = parse_ppm_u32(data, &mut pos)?;
    if width == 0 || height == 0 || maxval == 0 || maxval > 255 {
        return Err(DecodeError::UnsupportedDimension);
    }
    if pos >= data.len() || !data[pos].is_ascii_whitespace() {
        return Err(DecodeError::CorruptHeader);
    }
    pos += 1; // 1 whitespace byte before binary raster
    Ok((width, height, pos))
}

/// Decodes a PPM P6 binary frame into RGBA32 format.
/// Returns (width, height, next_frame_offset).
pub fn decode_ppm_frame(
    data: &[u8],
    out_rgba: &mut [u8],
) -> Result<(u32, u32, usize), DecodeError> {
    let (width, height, offset) = parse_ppm_header(data)?;
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(DecodeError::UnsupportedDimension)?;
    let required_rgb = pixel_count
        .checked_mul(3)
        .ok_or(DecodeError::UnsupportedDimension)?;
    let required_rgba = pixel_count
        .checked_mul(4)
        .ok_or(DecodeError::UnsupportedDimension)?;

    if data.len() < offset + required_rgb {
        return Err(DecodeError::BufferTooSmall);
    }
    if out_rgba.len() < required_rgba {
        return Err(DecodeError::BufferTooSmall);
    }

    let rgb = &data[offset..offset + required_rgb];
    for i in 0..pixel_count {
        out_rgba[i * 4] = rgb[i * 3];
        out_rgba[i * 4 + 1] = rgb[i * 3 + 1];
        out_rgba[i * 4 + 2] = rgb[i * 3 + 2];
        out_rgba[i * 4 + 3] = 255;
    }

    Ok((width, height, offset + required_rgb))
}

// -----------------------------------------------------------------------------
// RAW Frame Decoder
// -----------------------------------------------------------------------------

/// Header for RAW video frame container:
/// Magic: b"RAWF" (4 bytes)
/// Width: u32 big-endian (4 bytes)
/// Height: u32 big-endian (4 bytes)
/// Channels: u8 (3 for RGB, 4 for RGBA)
pub const RAW_HEADER_LEN: usize = 13;

pub fn parse_raw_header(data: &[u8]) -> Result<(u32, u32, bool, usize), DecodeError> {
    if data.len() < RAW_HEADER_LEN {
        return Err(DecodeError::BufferTooSmall);
    }
    if &data[0..4] != b"RAWF" {
        return Err(DecodeError::InvalidMagic);
    }
    let w = u32::from_be_bytes(data[4..8].try_into().map_err(|_| DecodeError::CorruptHeader)?);
    let h = u32::from_be_bytes(data[8..12].try_into().map_err(|_| DecodeError::CorruptHeader)?);
    let channels = data[12];
    if w == 0 || h == 0 || (channels != 3 && channels != 4) {
        return Err(DecodeError::UnsupportedDimension);
    }
    Ok((w, h, channels == 4, RAW_HEADER_LEN))
}

/// Decodes raw RGB or RGBA frame bytes into destination RGBA buffer.
pub fn decode_raw_frame(
    data: &[u8],
    width: u32,
    height: u32,
    is_rgba: bool,
    out_rgba: &mut [u8],
) -> Result<usize, DecodeError> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or(DecodeError::UnsupportedDimension)?;
    let required_rgba = pixel_count
        .checked_mul(4)
        .ok_or(DecodeError::UnsupportedDimension)?;
    let in_stride = if is_rgba { 4 } else { 3 };
    let required_in = pixel_count
        .checked_mul(in_stride)
        .ok_or(DecodeError::UnsupportedDimension)?;

    if data.len() < required_in || out_rgba.len() < required_rgba {
        return Err(DecodeError::BufferTooSmall);
    }

    if is_rgba {
        out_rgba[..required_rgba].copy_from_slice(&data[..required_rgba]);
    } else {
        for i in 0..pixel_count {
            out_rgba[i * 4] = data[i * 3];
            out_rgba[i * 4 + 1] = data[i * 3 + 1];
            out_rgba[i * 4 + 2] = data[i * 3 + 2];
            out_rgba[i * 4 + 3] = 255;
        }
    }
    Ok(required_rgba)
}

// -----------------------------------------------------------------------------
// MJPEG Frame Parser
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MjpegFrameInfo {
    pub start: usize,
    pub end: usize,
    pub width: u32,
    pub height: u32,
}

/// Scans for an MJPEG frame starting at or after `start_offset`.
/// Locates SOI (0xFFD8), reads dimensions from SOF0/SOF2 (0xFFC0/0xFFC2), and locates EOI (0xFFD9).
pub fn parse_mjpeg_frame(data: &[u8], start_offset: usize) -> Result<MjpegFrameInfo, DecodeError> {
    if data.len() < start_offset + 4 {
        return Err(DecodeError::EndOfStream);
    }

    // Find SOI (0xFF, 0xD8)
    let mut soi_pos = None;
    let mut i = start_offset;
    while i + 1 < data.len() {
        if data[i] == 0xFF && data[i + 1] == 0xD8 {
            soi_pos = Some(i);
            break;
        }
        i += 1;
    }
    let soi = soi_pos.ok_or(DecodeError::EndOfStream)?;

    let mut pos = soi + 2;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut eoi_pos = None;

    while pos + 1 < data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = data[pos + 1];
        if marker == 0xD9 {
            // EOI
            eoi_pos = Some(pos + 2);
            break;
        }
        if marker == 0xC0 || marker == 0xC2 {
            // SOF0 (Baseline) or SOF2 (Progressive)
            if pos + 9 <= data.len() {
                // marker(2) + len(2) + precision(1) + height(2) + width(2)
                height = ((data[pos + 5] as u32) << 8) | (data[pos + 6] as u32);
                width = ((data[pos + 7] as u32) << 8) | (data[pos + 8] as u32);
            }
        }
        pos += 2;
    }

    let end = eoi_pos.ok_or(DecodeError::BufferTooSmall)?;
    Ok(MjpegFrameInfo {
        start: soi,
        end,
        width,
        height,
    })
}

/// Counts total MJPEG frames in a buffer.
pub fn count_mjpeg_frames(data: &[u8]) -> usize {
    let mut count = 0;
    let mut offset = 0;
    while let Ok(info) = parse_mjpeg_frame(data, offset) {
        count += 1;
        offset = info.end;
        if offset >= data.len() {
            break;
        }
    }
    count
}

// -----------------------------------------------------------------------------
// H.264 / MP4 Truth Contract Handler
// -----------------------------------------------------------------------------

/// Enforces the Platform Truth Contract:
/// When an H.264 or MP4 stream is loaded without physical silicon decoder,
/// reports the exact truth contract error and refuses to fabricate pixels.
pub fn decode_h264_or_mp4_frame(_data: &[u8]) -> Result<(), &'static str> {
    Err(NO_HW_H264_MSG)
}

// -----------------------------------------------------------------------------
// Canvas Scaling & Mapping
// -----------------------------------------------------------------------------

pub const CANVAS_W: usize = 680;
pub const CANVAS_H: usize = 382;

/// Maps a coordinate `(cx, cy)` on the 680x382 canvas to `(sx, sy)` on the source frame,
/// according to the selected aspect ratio.
/// Returns `None` if `(cx, cy)` falls outside the active video frame (e.g. 4:3 letterbox bars).
pub fn map_canvas_to_source(
    cx: usize,
    cy: usize,
    src_w: u32,
    src_h: u32,
    aspect: AspectRatio,
) -> Option<(usize, usize)> {
    if cx >= CANVAS_W || cy >= CANVAS_H || src_w == 0 || src_h == 0 {
        return None;
    }

    match aspect {
        AspectRatio::Fit16_9 | AspectRatio::Fill => {
            let sx = ((cx as u64 * (src_w as u64 - 1)) / (CANVAS_W as u64 - 1).max(1)) as usize;
            let sy = ((cy as u64 * (src_h as u64 - 1)) / (CANVAS_H as u64 - 1).max(1)) as usize;
            Some((sx, sy))
        }
        AspectRatio::Aspect4_3 => {
            // 4:3 centered inside 16:9 canvas (CANVAS_H = 382)
            // width = 382 * 4 / 3 = 509 px. Lateral letterbox inset = (680 - 509) / 2 = 85 px.
            let active_w = (CANVAS_H * 4) / 3;
            let inset_x = (CANVAS_W.saturating_sub(active_w)) / 2;
            if cx < inset_x || cx >= inset_x + active_w {
                return None; // Inside black letterbox bar
            }
            let active_cx = cx - inset_x;
            let sx = ((active_cx as u64 * (src_w as u64 - 1)) / (active_w as u64 - 1).max(1)) as usize;
            let sy = ((cy as u64 * (src_h as u64 - 1)) / (CANVAS_H as u64 - 1).max(1)) as usize;
            Some((sx, sy))
        }
    }
}

// -----------------------------------------------------------------------------
// Scrubber & Time Formatting
// -----------------------------------------------------------------------------

/// Formats a time in seconds as `mm:ss` or `-mm:ss` (or `h:mm:ss`).
/// Returns bytes written.
pub fn format_time(seconds: u32, is_negative: bool, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    if is_negative && n < out.len() {
        out[n] = b'-';
        n += 1;
    }

    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;

    if h > 0 {
        if n < out.len() {
            out[n] = b'0' + ((h / 10) % 10) as u8;
            n += 1;
        }
        if n < out.len() {
            out[n] = b'0' + (h % 10) as u8;
            n += 1;
        }
        if n < out.len() {
            out[n] = b':';
            n += 1;
        }
    }

    if n < out.len() {
        out[n] = b'0' + ((m / 10) % 10) as u8;
        n += 1;
    }
    if n < out.len() {
        out[n] = b'0' + (m % 10) as u8;
        n += 1;
    }
    if n < out.len() {
        out[n] = b':';
        n += 1;
    }
    if n < out.len() {
        out[n] = b'0' + ((s / 10) % 10) as u8;
        n += 1;
    }
    if n < out.len() {
        out[n] = b'0' + (s % 10) as u8;
        n += 1;
    }

    n
}

/// Calculates the scrubber thumb X coordinate given track geometry.
pub fn scrubber_thumb_x(
    bar_x: usize,
    bar_w: usize,
    elapsed_sec: u32,
    duration_sec: u32,
) -> usize {
    if duration_sec == 0 || bar_w == 0 {
        return bar_x;
    }
    let clamped_elapsed = elapsed_sec.min(duration_sec);
    bar_x + ((clamped_elapsed as u64 * bar_w as u64) / duration_sec as u64) as usize
}

/// Translates a touch X position on the seek bar into elapsed seconds.
pub fn scrubber_seek_seconds(
    touch_x: usize,
    bar_x: usize,
    bar_w: usize,
    duration_sec: u32,
) -> u32 {
    if bar_w == 0 || duration_sec == 0 {
        return 0;
    }
    if touch_x <= bar_x {
        return 0;
    }
    if touch_x >= bar_x + bar_w {
        return duration_sec;
    }
    let rel_x = touch_x - bar_x;
    ((rel_x as u64 * duration_sec as u64) / bar_w as u64) as u32
}

// -----------------------------------------------------------------------------
// Playlist Management
// -----------------------------------------------------------------------------

pub const MAX_PLAYLIST_ITEMS: usize = 16;
pub const TITLE_CAP: usize = 48;
pub const PATH_CAP: usize = 96;

#[derive(Clone, Copy)]
pub struct PlaylistItem {
    pub title: [u8; TITLE_CAP],
    pub title_len: usize,
    pub path: [u8; PATH_CAP],
    pub path_len: usize,
    pub codec: VideoCodec,
    pub duration_sec: u32,
    pub width: u32,
    pub height: u32,
}

impl PlaylistItem {
    pub const EMPTY: Self = Self {
        title: [0; TITLE_CAP],
        title_len: 0,
        path: [0; PATH_CAP],
        path_len: 0,
        codec: VideoCodec::Unknown,
        duration_sec: 0,
        width: 0,
        height: 0,
    };

    pub fn title_str(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("Video")
    }

    pub fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

pub struct Playlist {
    pub items: [PlaylistItem; MAX_PLAYLIST_ITEMS],
    pub count: usize,
    pub current: usize,
}

impl Playlist {
    pub const fn new() -> Self {
        Self {
            items: [PlaylistItem::EMPTY; MAX_PLAYLIST_ITEMS],
            count: 0,
            current: 0,
        }
    }

    pub fn add(&mut self, item: PlaylistItem) -> bool {
        if self.count >= MAX_PLAYLIST_ITEMS {
            return false;
        }
        self.items[self.count] = item;
        self.count += 1;
        true
    }

    pub fn current_item(&self) -> Option<&PlaylistItem> {
        if self.count == 0 || self.current >= self.count {
            None
        } else {
            Some(&self.items[self.current])
        }
    }

    pub fn next(&mut self, loop_mode: LoopMode) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        match loop_mode {
            LoopMode::LoopOne => Some(self.current),
            LoopMode::LoopAll => {
                self.current = (self.current + 1) % self.count;
                Some(self.current)
            }
            LoopMode::None => {
                if self.current + 1 < self.count {
                    self.current += 1;
                    Some(self.current)
                } else {
                    None
                }
            }
        }
    }

    pub fn prev(&mut self) -> Option<usize> {
        if self.count == 0 {
            return None;
        }
        if self.current > 0 {
            self.current -= 1;
            Some(self.current)
        } else {
            self.current = self.count.saturating_sub(1);
            Some(self.current)
        }
    }
}

// -----------------------------------------------------------------------------
// Unit Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_truth_contract_h264_rejection() {
        let fake_mp4 = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00isommp42";
        let res = decode_h264_or_mp4_frame(fake_mp4);
        assert_eq!(res, Err(NO_HW_H264_MSG));
        assert_eq!(NO_HW_H264_MSG, "No hardware H.264 decoder bound. Software decode unavailable.");
    }

    #[test]
    fn test_video_suffix_detection() {
        assert!(has_video_suffix(b"sample.ppm"));
        assert!(has_video_suffix(b"CLIP.RAW"));
        assert!(has_video_suffix(b"stream.mjpg"));
        assert!(has_video_suffix(b"movie.mjpeg"));
        assert!(has_video_suffix(b"teaser.mp4"));
        assert!(has_video_suffix(b"test.h264"));
        assert!(!has_video_suffix(b"picture.png"));
        assert!(!has_video_suffix(b"audio.mp3"));
        assert!(!has_video_suffix(b""));
    }

    #[test]
    fn test_codec_detection_from_name_and_bytes() {
        assert_eq!(detect_codec_from_name(b"test.ppm"), VideoCodec::PpmP6);
        assert_eq!(detect_codec_from_name(b"test.raw"), VideoCodec::RawRgb);
        assert_eq!(detect_codec_from_name(b"test.mjpeg"), VideoCodec::Mjpeg);
        assert_eq!(detect_codec_from_name(b"test.mp4"), VideoCodec::H264);

        assert_eq!(detect_codec_from_bytes(b"P6\n10 10\n255\n"), VideoCodec::PpmP6);
        assert_eq!(detect_codec_from_bytes(b"\xFF\xD8\xFF\xE0"), VideoCodec::Mjpeg);
        assert_eq!(detect_codec_from_bytes(b"\x00\x00\x00\x14ftyp"), VideoCodec::H264);
    }

    #[test]
    fn test_ppm_p6_frame_decode() {
        let mut ppm_data = [0u8; 32];
        let header = b"P6\n2 2\n255\n";
        ppm_data[..header.len()].copy_from_slice(header);
        let pos = header.len();
        // 4 pixels RGB: Red, Green, Blue, White
        let pixels = [
            255, 0, 0,   // Red
            0, 255, 0,   // Green
            0, 0, 255,   // Blue
            255, 255, 255, // White
        ];
        ppm_data[pos..pos + pixels.len()].copy_from_slice(&pixels);

        let mut out = [0u8; 16];
        let (w, h, next) = decode_ppm_frame(&ppm_data, &mut out).expect("ppm decode failed");
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert_eq!(next, header.len() + 12);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]);
        assert_eq!(&out[4..8], &[0, 255, 0, 255]);
        assert_eq!(&out[8..12], &[0, 0, 255, 255]);
        assert_eq!(&out[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn test_raw_frame_decode() {
        let mut raw_file = [0u8; 13 + 6];
        raw_file[0..4].copy_from_slice(b"RAWF");
        raw_file[4..8].copy_from_slice(&2u32.to_be_bytes());
        raw_file[8..12].copy_from_slice(&1u32.to_be_bytes());
        raw_file[12] = 3; // RGB
        raw_file[13..19].copy_from_slice(&[10, 20, 30, 40, 50, 60]);

        let (w, h, is_rgba, offset) = parse_raw_header(&raw_file).expect("raw header parse");
        assert_eq!(w, 2);
        assert_eq!(h, 1);
        assert!(!is_rgba);
        assert_eq!(offset, 13);

        let mut out = [0u8; 8];
        let bytes = decode_raw_frame(&raw_file[offset..], w, h, is_rgba, &mut out)
            .expect("raw decode");
        assert_eq!(bytes, 8);
        assert_eq!(&out[0..4], &[10, 20, 30, 255]);
        assert_eq!(&out[4..8], &[40, 50, 60, 255]);
    }

    #[test]
    fn test_mjpeg_marker_parsing() {
        let mut jpeg_stream = [0u8; 32];
        jpeg_stream[0] = 0xFF;
        jpeg_stream[1] = 0xD8; // SOI
        jpeg_stream[2] = 0xFF;
        jpeg_stream[3] = 0xC0; // SOF0
        jpeg_stream[4] = 0x00;
        jpeg_stream[5] = 0x11; // Length
        jpeg_stream[6] = 0x08; // Precision 8
        jpeg_stream[7] = 0x01;
        jpeg_stream[8] = 0x80; // Height: 384
        jpeg_stream[9] = 0x02;
        jpeg_stream[10] = 0xA0; // Width: 672
        jpeg_stream[11] = 0xFF;
        jpeg_stream[12] = 0xD9; // EOI

        let info = parse_mjpeg_frame(&jpeg_stream, 0).expect("mjpeg frame parse failed");
        assert_eq!(info.start, 0);
        assert_eq!(info.end, 13);
        assert_eq!(info.height, 384);
        assert_eq!(info.width, 672);

        assert_eq!(count_mjpeg_frames(&jpeg_stream[..13]), 1);
    }

    #[test]
    fn test_playback_state_machine() {
        let mut state = PlaybackState::Stopped;
        assert_eq!(state.label(), "STOPPED");
        state = PlaybackState::Playing;
        assert_eq!(state.label(), "PLAYING");
        state = PlaybackState::Paused;
        assert_eq!(state.label(), "PAUSED");
    }

    #[test]
    fn test_aspect_ratio_and_loop_cycling() {
        let mut ar = AspectRatio::Fit16_9;
        assert_eq!(ar.label(), "16:9");
        ar = ar.next();
        assert_eq!(ar.label(), "4:3");
        ar = ar.next();
        assert_eq!(ar.label(), "FILL");
        ar = ar.next();
        assert_eq!(ar.label(), "16:9");

        let mut lm = LoopMode::None;
        assert_eq!(lm.label(), "OFF");
        lm = lm.next();
        assert_eq!(lm.label(), "ONE");
        lm = lm.next();
        assert_eq!(lm.label(), "ALL");
        lm = lm.next();
        assert_eq!(lm.label(), "OFF");
    }

    #[test]
    fn test_canvas_mapping_letterbox() {
        // Fit 16:9 mapping
        let pt = map_canvas_to_source(340, 191, 1920, 1080, AspectRatio::Fit16_9);
        assert!(pt.is_some());

        // 4:3 Aspect mapping has pillarbox bars
        // For CANVAS_W = 680, active 4:3 area width = 509, inset = 85.
        // cx = 10 is inside lateral bar -> None
        let bar_pt = map_canvas_to_source(10, 100, 640, 480, AspectRatio::Aspect4_3);
        assert_eq!(bar_pt, None);

        // cx = 340 is center of 4:3 content -> Some
        let center_pt = map_canvas_to_source(340, 100, 640, 480, AspectRatio::Aspect4_3);
        assert!(center_pt.is_some());
    }

    #[test]
    fn test_time_formatting() {
        let mut buf = [0u8; 16];
        let len = format_time(135, false, &mut buf); // 2m 15s -> "02:15"
        assert_eq!(&buf[..len], b"02:15");

        let mut buf_neg = [0u8; 16];
        let len_neg = format_time(272, true, &mut buf_neg); // -4m 32s -> "-04:32"
        assert_eq!(&buf_neg[..len_neg], b"-04:32");
    }

    #[test]
    fn test_scrubber_calculations() {
        let thumb_x = scrubber_thumb_x(100, 500, 60, 120);
        assert_eq!(thumb_x, 350); // exactly midpoint

        let seek_sec = scrubber_seek_seconds(350, 100, 500, 120);
        assert_eq!(seek_sec, 60);

        let seek_clamp_min = scrubber_seek_seconds(50, 100, 500, 120);
        assert_eq!(seek_clamp_min, 0);

        let seek_clamp_max = scrubber_seek_seconds(700, 100, 500, 120);
        assert_eq!(seek_clamp_max, 120);
    }

    #[test]
    fn test_playlist_navigation() {
        let mut pl = Playlist::new();
        assert_eq!(pl.current_item().is_none(), true);

        let mut item1 = PlaylistItem::EMPTY;
        item1.title_len = 5;
        item1.title[..5].copy_from_slice(b"Clip1");
        item1.duration_sec = 60;
        pl.add(item1);

        let mut item2 = PlaylistItem::EMPTY;
        item2.title_len = 5;
        item2.title[..5].copy_from_slice(b"Clip2");
        item2.duration_sec = 120;
        pl.add(item2);

        assert_eq!(pl.count, 2);
        assert_eq!(pl.current_item().unwrap().title_str(), "Clip1");

        pl.next(LoopMode::None);
        assert_eq!(pl.current_item().unwrap().title_str(), "Clip2");

        pl.next(LoopMode::LoopAll);
        assert_eq!(pl.current_item().unwrap().title_str(), "Clip1");

        pl.prev();
        assert_eq!(pl.current_item().unwrap().title_str(), "Clip2");
    }
}
