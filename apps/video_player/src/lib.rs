// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! InterOS Native Video Player Engine (Ring-3)
//!
//! Provides the core media state machine, frame decoders (RAW, PPM P6, MJPEG),
//! a bounded RIFF/AVI + Motion JPEG reader, codec detection, scrubber
//! calculations, aspect ratio mapping, playback speeds, orientation modes,
//! volume control, and platform truth contract enforcement.

#![no_std]

#[cfg(test)]
extern crate std as host_std;

pub use interos_std as std;

pub mod avi;
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
    /// RIFF AVI container. The video stream inside it is read by `avi`; only
    /// Motion JPEG payloads in it can be decoded.
    Avi,
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
            VideoCodec::Avi => "AVI",
            VideoCodec::H264 => "H.264",
            VideoCodec::Unknown => "UNKNOWN",
        }
    }

    /// Whether this format carries a running time the player could advance.
    ///
    /// WHY this exists: the media library is the platform's inventory, not this
    /// app's, and it hands back whatever a directory holds -- including a
    /// Portable Pixmap, which is one still frame and has no clock to run. The
    /// transport, the scrubber and the elapsed digits all have to answer "this
    /// is not time-based media" for those instead of drawing a paused clip that
    /// never was one.
    pub const fn is_still_image(self) -> bool {
        matches!(self, VideoCodec::PpmP6 | VideoCodec::RawRgb | VideoCodec::RawRgba)
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
    } else if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".avi") {
        VideoCodec::Avi
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
/// Cycles through Fit, Fill, Stretch, 16:9, 4:3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatio {
    /// Fits source within canvas bounds preserving aspect ratio (pillarbox or letterbox).
    Fit,
    /// Centers and zooms to completely fill canvas, cropping excess source edges.
    Fill,
    /// Stretched to completely fill the canvas bounds regardless of source aspect ratio.
    Stretch,
    /// 16:9 widescreen canvas standard format.
    Fit16_9,
    /// 4:3 pillarboxed with black lateral bars.
    Aspect4_3,
}

impl AspectRatio {
    pub const fn label(self) -> &'static str {
        match self {
            AspectRatio::Fit => "FIT",
            AspectRatio::Fill => "FILL",
            AspectRatio::Stretch => "STRETCH",
            AspectRatio::Fit16_9 => "16:9",
            AspectRatio::Aspect4_3 => "4:3",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            AspectRatio::Fit => AspectRatio::Fill,
            AspectRatio::Fill => AspectRatio::Stretch,
            AspectRatio::Stretch => AspectRatio::Fit16_9,
            AspectRatio::Fit16_9 => AspectRatio::Aspect4_3,
            AspectRatio::Aspect4_3 => AspectRatio::Fit,
        }
    }
}

/// Playback speed selector supporting 0.5x, 0.75x, 1.0x, 1.25x, 1.5x, 2.0x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSpeed {
    Speed0_5x = 0,
    Speed0_75x = 1,
    Speed1_0x = 2,
    Speed1_25x = 3,
    Speed1_5x = 4,
    Speed2_0x = 5,
}

impl PlaybackSpeed {
    pub const fn label(self) -> &'static str {
        match self {
            PlaybackSpeed::Speed0_5x => "0.5x",
            PlaybackSpeed::Speed0_75x => "0.75x",
            PlaybackSpeed::Speed1_0x => "1.0x",
            PlaybackSpeed::Speed1_25x => "1.25x",
            PlaybackSpeed::Speed1_5x => "1.5x",
            PlaybackSpeed::Speed2_0x => "2.0x",
        }
    }

    pub const fn multiplier_milli(self) -> u32 {
        match self {
            PlaybackSpeed::Speed0_5x => 500,
            PlaybackSpeed::Speed0_75x => 750,
            PlaybackSpeed::Speed1_0x => 1000,
            PlaybackSpeed::Speed1_25x => 1250,
            PlaybackSpeed::Speed1_5x => 1500,
            PlaybackSpeed::Speed2_0x => 2000,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            PlaybackSpeed::Speed0_5x => PlaybackSpeed::Speed0_75x,
            PlaybackSpeed::Speed0_75x => PlaybackSpeed::Speed1_0x,
            PlaybackSpeed::Speed1_0x => PlaybackSpeed::Speed1_25x,
            PlaybackSpeed::Speed1_25x => PlaybackSpeed::Speed1_5x,
            PlaybackSpeed::Speed1_5x => PlaybackSpeed::Speed2_0x,
            PlaybackSpeed::Speed2_0x => PlaybackSpeed::Speed0_5x,
        }
    }
}

/// Screen presentation orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

impl Orientation {
    pub const fn label(self) -> &'static str {
        match self {
            Orientation::Portrait => "PORTRAIT",
            Orientation::Landscape => "LANDSCAPE",
        }
    }

    pub const fn toggle(self) -> Self {
        match self {
            Orientation::Portrait => Orientation::Landscape,
            Orientation::Landscape => Orientation::Portrait,
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

    /// The words the loop control paints: the feature and the state together.
    ///
    /// WHY a second face and not a changed [`label`]: `label()` is the state
    /// name the platform truth contract pins, and it is what a log line and the
    /// properties sheet quote. A chip that reads only `OFF` names no function --
    /// a person looking at `1.0x  FIT  OFF  PIP` cannot tell that the third one
    /// repeats the playlist -- so the plate paints the feature word with the
    /// state on it, and the state name itself stays where it was.
    pub const fn chip_label(self) -> &'static str {
        match self {
            LoopMode::None => "LOOP OFF",
            LoopMode::LoopOne => "LOOP ONE",
            LoopMode::LoopAll => "LOOP ALL",
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

/// Maps a coordinate `(cx, cy)` on an arbitrary canvas bounds `(canvas_w, canvas_h)`
/// to `(sx, sy)` on the source frame, according to the selected aspect ratio mode.
/// Returns `None` if `(cx, cy)` falls outside the active video frame (e.g. letterbox/pillarbox bars).
pub fn map_coords_to_source(
    cx: usize,
    cy: usize,
    canvas_w: usize,
    canvas_h: usize,
    src_w: u32,
    src_h: u32,
    aspect: AspectRatio,
) -> Option<(usize, usize)> {
    if cx >= canvas_w || cy >= canvas_h || src_w == 0 || src_h == 0 {
        return None;
    }

    match aspect {
        AspectRatio::Stretch => {
            let sx = ((cx as u64 * (src_w as u64 - 1)) / (canvas_w as u64 - 1).max(1)) as usize;
            let sy = ((cy as u64 * (src_h as u64 - 1)) / (canvas_h as u64 - 1).max(1)) as usize;
            Some((sx, sy))
        }
        AspectRatio::Fit16_9 => {
            // Force 16:9 bounds inside canvas
            let (target_w, target_h, off_x, off_y) = if canvas_w * 9 > canvas_h * 16 {
                let tw = (canvas_h * 16) / 9;
                (tw, canvas_h, (canvas_w.saturating_sub(tw)) / 2, 0)
            } else {
                let th = (canvas_w * 9) / 16;
                (canvas_w, th, 0, (canvas_h.saturating_sub(th)) / 2)
            };
            if cx < off_x || cx >= off_x + target_w || cy < off_y || cy >= off_y + target_h {
                return None;
            }
            let active_x = cx - off_x;
            let active_y = cy - off_y;
            let sx = ((active_x as u64 * (src_w as u64 - 1)) / (target_w as u64 - 1).max(1)) as usize;
            let sy = ((active_y as u64 * (src_h as u64 - 1)) / (target_h as u64 - 1).max(1)) as usize;
            Some((sx, sy))
        }
        AspectRatio::Aspect4_3 => {
            // Force 4:3 bounds inside canvas
            let (target_w, target_h, off_x, off_y) = if canvas_w * 3 > canvas_h * 4 {
                let tw = (canvas_h * 4) / 3;
                (tw, canvas_h, (canvas_w.saturating_sub(tw)) / 2, 0)
            } else {
                let th = (canvas_w * 3) / 4;
                (canvas_w, th, 0, (canvas_h.saturating_sub(th)) / 2)
            };
            if cx < off_x || cx >= off_x + target_w || cy < off_y || cy >= off_y + target_h {
                return None;
            }
            let active_x = cx - off_x;
            let active_y = cy - off_y;
            let sx = ((active_x as u64 * (src_w as u64 - 1)) / (target_w as u64 - 1).max(1)) as usize;
            let sy = ((active_y as u64 * (src_h as u64 - 1)) / (target_h as u64 - 1).max(1)) as usize;
            Some((sx, sy))
        }
        AspectRatio::Fit => {
            // Maintain source aspect ratio within canvas bounds (letterbox or pillarbox)
            let sw = src_w as usize;
            let sh = src_h as usize;
            let (target_w, target_h, off_x, off_y) = if canvas_w * sh > canvas_h * sw {
                let tw = (canvas_h * sw) / sh;
                (tw, canvas_h, (canvas_w.saturating_sub(tw)) / 2, 0)
            } else {
                let th = (canvas_w * sh) / sw;
                (canvas_w, th, 0, (canvas_h.saturating_sub(th)) / 2)
            };
            if cx < off_x || cx >= off_x + target_w || cy < off_y || cy >= off_y + target_h {
                return None;
            }
            let active_x = cx - off_x;
            let active_y = cy - off_y;
            let sx = ((active_x as u64 * (src_w as u64 - 1)) / (target_w as u64 - 1).max(1)) as usize;
            let sy = ((active_y as u64 * (src_h as u64 - 1)) / (target_h as u64 - 1).max(1)) as usize;
            Some((sx, sy))
        }
        AspectRatio::Fill => {
            // Zoom/crop to fill canvas completely
            let sw = src_w as u64;
            let sh = src_h as u64;
            let cw = canvas_w as u64;
            let ch = canvas_h as u64;
            if cw * sh > ch * sw {
                let crop_h = (sw * ch) / cw;
                let crop_y0 = (sh.saturating_sub(crop_h)) / 2;
                let sx = ((cx as u64 * (sw - 1)) / (cw - 1).max(1)) as usize;
                let sy = (crop_y0 + (cy as u64 * (crop_h - 1)) / (ch - 1).max(1)) as usize;
                Some((sx, sy))
            } else {
                let crop_w = (sh * cw) / ch;
                let crop_x0 = (sw.saturating_sub(crop_w)) / 2;
                let sx = (crop_x0 + (cx as u64 * (crop_w - 1)) / (cw - 1).max(1)) as usize;
                let sy = ((cy as u64 * (sh - 1)) / (ch - 1).max(1)) as usize;
                Some((sx, sy))
            }
        }
    }
}

/// Backward compatible canvas mapping for standard 680x382 portrait video surface.
pub fn map_canvas_to_source(
    cx: usize,
    cy: usize,
    src_w: u32,
    src_h: u32,
    aspect: AspectRatio,
) -> Option<(usize, usize)> {
    map_coords_to_source(cx, cy, CANVAS_W, CANVAS_H, src_w, src_h, aspect)
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

/// The words the scrubber row stands in place of a track bar when the media
/// carries no running time at all.
///
/// WHY a sentence and not a dash cluster: the row used to paint `00:00` beside
/// `--:--`, which a person reads as "a clip, paused at the start". It is neither
/// -- there is no clock to pause -- and the mark itself is two hyphens, a colon
/// and two more hyphens, which at the body rung's cap height does not read as a
/// number or as a word. It reads as a glyph the font failed to draw.
pub const NO_RUNNING_TIME: &[u8] = b"NO RUNNING TIME";

/// The short form of [`NO_RUNNING_TIME`], for the columns a list row has.
///
/// WHY a second spelling: a library row gives its duration field the width left
/// over by the size, the codec and the frame size it quotes first, and the
/// eleven columns of the full sentence are not in it.
pub const NO_TIME_SHORT: &[u8] = b"NO TIME";

/// In `out`, the running time a known duration names, or the words that say no
/// running time was ever read.
///
/// WHY the words exist: a duration of zero is what a header nobody has read --
/// or a still image, which has no header to read -- yields, and drawing `0:00`
/// for it would claim the media is empty instead of reporting that its length is
/// not a thing this format has.
pub fn duration_label(known: bool, seconds: u32, negative: bool, out: &mut [u8]) -> usize {
    if !known {
        let marker = if out.len() >= NO_RUNNING_TIME.len() {
            NO_RUNNING_TIME
        } else {
            NO_TIME_SHORT
        };
        out[..marker.len()].copy_from_slice(marker);
        return marker.len();
    }
    format_time(seconds, negative, out)
}

/// In `out`, what the system's audio output level is, in words.
///
/// WHY words: the read-out used to answer a level nobody reported with `--`, two
/// hyphens on the page colour at the body rung, which is the same broken-glyph
/// look as the scrubber's marker. Three states, three words, and none of them a
/// mark the face has to be missing to produce.
///
/// WHY `UNKNOWN` rather than a zero: a failed `audio_query_volume` is not a
/// muted output, and painting `0%` would report a level the system never gave.
pub fn volume_readout(percent: u32, muted: bool, known: bool, out: &mut [u8]) -> usize {
    if !known {
        return put_word(UNKNOWN_LEVEL, out);
    }
    if muted {
        return put_word(MUTED, out);
    }
    let mut n = 0usize;
    let mut digits = [0u8; 4];
    let mut dlen = 0usize;
    let mut v = percent.min(100);
    if v == 0 {
        digits[0] = b'0';
        dlen = 1;
    } else {
        while v > 0 && dlen < 4 {
            digits[dlen] = b'0' + (v % 10) as u8;
            v /= 10;
            dlen += 1;
        }
    }
    while n < dlen && n + 2 < out.len() {
        out[n] = digits[dlen - 1 - n];
        n += 1;
    }
    if n + 1 < out.len() {
        out[n] = b'%';
        n += 1;
    }
    n
}

/// The words [`volume_readout`] answers when the system never reported a level.
pub const UNKNOWN_LEVEL: &[u8] = b"UNKNOWN";

/// The words [`volume_readout`] answers while the output is muted.
pub const MUTED: &[u8] = b"MUTED";

/// Every word the level read-out can answer with: the two states and the widest
/// number it reports at the ceiling.
pub const VOLUME_READOUT_WORDS: [&[u8]; 3] = [UNKNOWN_LEVEL, MUTED, b"100%"];

/// The column the surface gives the level read-out: three line boxes of the body
/// rung at the largest level the setting can hold.
///
/// WHY authored here and read back by `main.rs`: the same reason
/// [`CHIP_PLATE_ROOM`] is. The column is the number the read-out's words are
/// sized against, and a copy kept inside `mod tests` -- where this one lived --
/// is a number the screen never reads. `main.rs` derives `VOL_LABEL_COL` from
/// the same two SDK faces and its compile-time gate says the two are one.
pub const VOLUME_READOUT_COL: usize =
    3 * std::design::font::line_height_for_user_scale(2, std::design::font::TEXT_SCALE_EXTRA_LARGE);

/// The two rungs the read-out may stand on, largest first -- the ladder the
/// painter hands [`label_rung`] at the plate row.
const READOUT_RUNGS: [usize; 2] = [
    std::design::typography::BODY_SCALE,
    std::design::typography::CAPTION_SCALE,
];

/// The advance `text` takes at `scale` on `level`, in columns.
///
/// WHY this re-adds the face's advances instead of calling `label_w`: the
/// measurement is the same sum the runtime face performs -- each glyph's advance
/// scaled by the em, integer-divided, then totalled -- but `label_w` reaches the
/// outline tables through a cached lookup that a `const` cannot make. It is
/// spelled over the base table only, which is the table every word in
/// [`VOLUME_READOUT_WORDS`] sits in; a byte outside it fails the gate rather
/// than quietly measuring nothing.
const fn readout_run_w(text: &[u8], scale: usize, level: u8) -> usize {
    let em = std::design::font::em_for_user_scale(scale, level);
    let upem = std::design::face::UNITS_PER_EM as u32;
    let mut total = 0usize;
    let mut i = 0usize;
    while i < text.len() {
        let b = text[i];
        if b < std::design::face::FIRST_CHAR || b > std::design::face::LAST_CHAR {
            return usize::MAX;
        }
        let adv = std::design::face::ADVANCE[(b - std::design::face::FIRST_CHAR) as usize] as u32;
        total += ((adv * em) / upem) as usize;
        i += 1;
    }
    total
}

/// WHAT THIS GATES: every word the level read-out can answer with stands inside
/// [`VOLUME_READOUT_COL`] at every level the setting can hold -- on the narrowest
/// rung the ladder offers at every level, and on the body rung at the platform's
/// own level, so the word a person sees by default is never the stepped one.
///
/// WHY a compile-time gate beside the library's runtime test:
/// `test_volume_readout_words_are_never_a_dash` inks the same words through the
/// runtime face, and that test is not in the binary the phone runs. `main.rs`
/// calls this from a `const`, so a column that stopped holding its own words
/// fails the build rather than showing up as a clipped last letter.
///
/// WHY the narrowest rung is the claim: the step down is what keeps a word on the
/// screen at a raised text size, so the rung that can still fail is the last one
/// it is allowed to take. A word that does not fit even there is a word painted
/// past its column, and there is no rung left to save it.
pub const fn volume_readout_words_hold_at_every_level() -> bool {
    let refuge = READOUT_RUNGS.len() - 1;
    let mut level = 0u8;
    while level <= std::design::font::TEXT_SCALE_EXTRA_LARGE {
        let mut w = 0usize;
        while w < VOLUME_READOUT_WORDS.len() {
            let text = VOLUME_READOUT_WORDS[w];
            if readout_run_w(text, READOUT_RUNGS[refuge], level) > VOLUME_READOUT_COL {
                return false;
            }
            if level == std::design::font::TEXT_SCALE_NORMAL
                && readout_run_w(text, READOUT_RUNGS[0], level) > VOLUME_READOUT_COL
            {
                return false;
            }
            w += 1;
        }
        level += 1;
    }
    true
}

fn put_word(word: &[u8], out: &mut [u8]) -> usize {
    let n = word.len().min(out.len());
    out[..n].copy_from_slice(&word[..n]);
    n
}

// -----------------------------------------------------------------------------
// One Run, One Plate: the rung a label is drawn at
// -----------------------------------------------------------------------------

/// The room a control plate gives its centred label: the plate's own width less
/// the air it keeps at each end.
///
/// WHY one number in the library rather than a spelling in each target: the bin
/// fits a run into this room when it paints a plate, and the host test fits
/// words into it to prove a label cannot overflow. The bin carries `test =
/// false`, so a second copy authored inside `mod tests` would be a number the
/// screen never reads -- and `main.rs`'s compile-time gate beside
/// `CHIP_LABEL_AIR` is what keeps this room equal to the plate it is derived
/// from.
pub const CHIP_PLATE_ROOM: usize = 154 - 2 * 4;

/// The advance a run takes at `scale` on `level`, in the weight it is drawn in.
///
/// WHY this is the measurement and not `font::measure`: `measure` is the bitmap
/// cell's arithmetic, and the screen's `draw_text*` faces answer the wider of
/// the outline run and that cell. This is the same expression the painter's
/// `run_w` reads, so a fit decided here is the fit the pixel path agrees with.
pub fn label_w(text: &[u8], bold: bool, scale: usize, level: u8) -> usize {
    if bold {
        std::design::font::measure_bold_user_scale(text, scale, level)
    } else {
        std::design::font::measure_user_scale(text, scale, level)
    }
}

/// The largest rung in `rungs`, first rung first, that `text` stands inside at
/// `level`; the last one when none of them does.
///
/// WHY a rung ladder and not a clipped run: a control's word that will not fit
/// its plate at a raised text size is still the word that names the control, and
/// `...` on `LOOP ONE` says `LOOP O...` -- a label a person cannot finish. The
/// fleet's own answer to this is the camera's refusal caption, which steps its
/// rung down rather than inking the ring beside it. The same step here keeps the
/// whole word, on the plate, at every level the setting holds.
pub fn label_rung(text: &[u8], bold: bool, max_w: usize, level: u8, rungs: &[usize]) -> usize {
    for &rung in rungs {
        if label_w(text, bold, rung, level) <= max_w {
            return rung;
        }
    }
    match rungs.last() {
        Some(&last) => last,
        None => 0,
    }
}

// -----------------------------------------------------------------------------
// The transport band's three words
// -----------------------------------------------------------------------------
//
// WHY the words are the library's and not the painter's: the band that holds
// Prev, Play/Pause and Next has always inked three discs and nothing else, so on
// a phone nobody has used before the only thing a person has to go on is that
// three round marks in a row do *something* -- and a mark whose meaning has to be
// guessed at is the same defect class as the mute plate that used to carry a red
// cross. Every other control on this screen states its word (`LOOP OFF`,
// `LIBRARY`, `MEDIA INFO`), so the transport states its three from the same
// derivation: [`label_rung`] stepped between [`PLATE_RUNGS`], centred by
// `paint_plate_label`, into the plate whose room is one of the two constants
// below.
//
// WHY they live here rather than in `main.rs`: the bin target carries
// `test = false`, so the only place a word can be measured against the room it is
// painted into is this file, where the host test below inks the ladder directly.

/// The word under the control that goes back to the start of the clip, or to the
/// clip before it when the list holds one.
///
/// WHY `PREV` and not `RESTART`: the list is asked first -- `Playlist::prev`
/// steps for every codec and `load_current_track` re-dispatches from the entry's
/// own header, so the file before this one is opened, not this one re-read. The
/// restart is the same mark's other half, the one every player answers within the
/// first seconds of a clip, and with a single track in the list the previous
/// position of the list *is* its first second.
pub const PREV_WORD: &[u8] = b"PREV";
/// The word under the control that steps to the next track the list holds.
pub const NEXT_WORD: &[u8] = b"NEXT";
/// The word under the play control while it is about to start the clip running.
pub const PLAY_WORD: &[u8] = b"PLAY";
/// The word under the play control while it is about to stop the clock.
pub const PAUSE_WORD: &[u8] = b"PAUSE";

/// The word the play control paints, for the action its own press performs.
///
/// WHY it is asked for by the same predicate the disc's glyph is drawn from: the
/// disc shows the bars while the clip runs and the triangle while it does not, so
/// a word that read `PLAY` under the bars would name the action the press does
/// not perform -- which is exactly the defect class this band is being fixed for.
pub const fn transport_action_word(playing: bool) -> &'static [u8] {
    if playing {
        PAUSE_WORD
    } else {
        PLAY_WORD
    }
}

/// The columns the portrait transport row leaves a control's word: the distance
/// between two neighbouring disc centres, less the air every plate label keeps at
/// each end.
///
/// WHY the centre distance and not the disc: the word stands under the whole
/// control, and the limit on it is the neighbour's word, not its own circle. Two
/// runs centred on their own controls cannot meet while each stays inside half
/// the distance between the centres, so the plate the painter fills is exactly
/// `180` columns wide and the room inside it is what a word is measured against.
pub const TRANSPORT_WORD_ROOM: usize = 180 - 2 * 4;

/// [`TRANSPORT_WORD_ROOM`] for the landscape scrim, whose three controls stand
/// `130` columns apart.
pub const LAND_TRANSPORT_WORD_ROOM: usize = 130 - 2 * 4;

// -----------------------------------------------------------------------------
// The speaker glyph, on the platform's icon grid
// -----------------------------------------------------------------------------
//
// WHY the geometry lives here and not in the painter: every screen in this
// repository answers "what colour is pixel (x, y)", so a glyph's shape is a
// property of arithmetic, and arithmetic this screen holds privately in a band
// of `get_video_pixel` cannot be tested -- the bin target carries `test = false`.
// The dialer's handset went through the same change for the same reason, and the
// gate that now holds it shut re-derives the glyph's own expressions. Here the
// host test can ink the grid directly.
//
// WHY the twenty-four unit grid: the platform's monochrome kit draws every icon
// on it, because a glyph authored in pixels is a different shape at every size.
// The kit's own `Icon` enum carries a mic, a bell and a play triangle, and no
// speaker, so this is the kit's grid and the kit's method with one glyph added
// by the app that needs it.

/// The side of the grid a glyph is drawn on, in grid units.
pub const ICON_GRID: i32 = 24;

/// What the audio output is doing, as far as this player has been told.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpeakerState {
    /// The system reported a level and the output is live: the horn carries
    /// waves.
    On,
    /// The person muted it: the same space the waves take carries a strike.
    Muted,
    /// The volume query was never answered, so the app does not know which of
    /// the two it is looking at. The horn is drawn alone -- no waves, because
    /// nothing reported sound coming out, and no strike, because nothing said
    /// it was shut off.
    Unknown,
}

impl SpeakerState {
    /// The state the volume fields describe.
    ///
    /// WHY mute answers first: a person who pressed the plate knows the output is
    /// shut, and that is true whatever the last query reported; the unknown state
    /// is only for a level nobody ever read.
    pub const fn from_volume(muted: bool, known: bool) -> Self {
        if muted {
            SpeakerState::Muted
        } else if !known {
            SpeakerState::Unknown
        } else {
            SpeakerState::On
        }
    }
}

/// Whether the grid point `(gx, gy)` is the horn: the magnet's bar and the mouth
/// it widens into.
pub fn speaker_horn_hit(gx: i32, gy: i32) -> bool {
    if gx < 3 || gx > 14 {
        return false;
    }
    let dy = (gy - 12).abs();
    if gx <= 8 {
        dy <= 3
    } else {
        dy <= gx - 6
    }
}

/// Whether the grid point is one sound wave: the arc of radius `r` the horn's
/// mouth faces, in front of it.
pub fn speaker_wave_hit(gx: i32, gy: i32, r: i32) -> bool {
    let dx = gx - 13;
    let dy = gy - 12;
    if dx <= 0 {
        return false;
    }
    let d2 = dx * dx + dy * dy;
    d2 >= (r - 1) * (r - 1) && d2 <= (r + 1) * (r + 1)
}

/// Whether the grid point is the strike a muted output carries, in the same
/// space the waves stand in when the output is live.
///
/// WHY it sits in front of the mouth and not across the whole glyph: the strike
/// answers the waves, so it takes their place; drawn corner to corner it would
/// cut the horn itself, and a person reading the plate is then looking at a
/// crossed-out speaker rather than a speaker with nothing coming out of it.
/// Its two endpoints keep the stroke's whole thickness inside the grid.
pub fn speaker_strike_hit(gx: i32, gy: i32) -> bool {
    segment_hit(gx, gy, 16, 6, 21, 18, 2)
}

/// Whether the grid point is inked by `state`'s speaker.
pub fn speaker_hit(gx: i32, gy: i32, state: SpeakerState) -> bool {
    if speaker_horn_hit(gx, gy) {
        return true;
    }
    match state {
        SpeakerState::On => speaker_wave_hit(gx, gy, 4) || speaker_wave_hit(gx, gy, 7),
        SpeakerState::Muted => speaker_strike_hit(gx, gy),
        SpeakerState::Unknown => false,
    }
}

/// Whether the grid point lies within `t` units of the segment `(x0, y0)` to
/// `(x1, y1)`.
///
/// WHY integers: the guest paints one pixel at a time inside a frame budget, and
/// this runs for every pixel of every plate on the screen. The projection is
/// carried in per-mille units so no division by the segment's own length loses
/// the position.
fn segment_hit(gx: i32, gy: i32, x0: i32, y0: i32, x1: i32, y1: i32, t: i32) -> bool {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len2 = dx * dx + dy * dy;
    if len2 == 0 {
        let ddx = gx - x0;
        let ddy = gy - y0;
        return ddx * ddx + ddy * ddy <= t * t;
    }
    let mut u = ((gx - x0) * dx + (gy - y0) * dy) * 1000 / len2;
    if u < 0 {
        u = 0;
    }
    if u > 1000 {
        u = 1000;
    }
    let cx = x0 + u * dx / 1000;
    let cy = y0 + u * dy / 1000;
    let ddx = gx - cx;
    let ddy = gy - cy;
    ddx * ddx + ddy * ddy <= t * t
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

/// Returns true if the frame at `frame_index` is an intra/keyframe.
/// For intra-only formats (Motion JPEG, PPM P6, RAW RGB), every frame is a keyframe.
pub fn is_keyframe(codec: VideoCodec, _frame_index: u32) -> bool {
    match codec {
        VideoCodec::Mjpeg
        | VideoCodec::Avi
        | VideoCodec::PpmP6
        | VideoCodec::RawRgb
        | VideoCodec::RawRgba => true,
        VideoCodec::H264 => false, // Inter-frame compression; requires IDR/keyframe index
        VideoCodec::Unknown => false,
    }
}

/// Computes the keyframe seek target second.
/// For intra-only formats (like MJPEG and PPM), every frame is an intra/keyframe,
/// so seeking aligns directly with the target frame.
pub fn scrubber_seek_keyframe(seek_sec: u32, duration_sec: u32, codec: VideoCodec) -> u32 {
    let _ = codec;
    seek_sec.min(duration_sec)
}

// -----------------------------------------------------------------------------
// Audio Volume Helpers
// -----------------------------------------------------------------------------

/// Calculates thumb X position for volume bar (0..100%).
pub fn volume_thumb_x(bar_x: usize, bar_w: usize, volume_percent: u32) -> usize {
    if bar_w == 0 {
        return bar_x;
    }
    let clamped = volume_percent.min(100);
    bar_x + ((clamped as usize * bar_w) / 100)
}

/// Translates a touch X position on the volume bar into volume percentage (0..100).
pub fn volume_seek_percent(touch_x: usize, bar_x: usize, bar_w: usize) -> u32 {
    if bar_w == 0 || touch_x <= bar_x {
        return 0;
    }
    if touch_x >= bar_x + bar_w {
        return 100;
    }
    let rel_x = touch_x - bar_x;
    ((rel_x as u32 * 100) / bar_w as u32).min(100)
}

// -----------------------------------------------------------------------------
// Metadata String Formatters
// -----------------------------------------------------------------------------

/// Formats a file size in bytes as human-readable ASCII: e.g. "512 B", "420 KB", "1.2 MB".
pub fn format_file_size(bytes: u64, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if bytes < 1024 {
        let mut digits = [0u8; 20];
        let mut d_len = 0;
        let mut val = bytes;
        if val == 0 {
            digits[0] = b'0';
            d_len = 1;
        } else {
            while val > 0 && d_len < 20 {
                digits[d_len] = b'0' + (val % 10) as u8;
                val /= 10;
                d_len += 1;
            }
        }
        let mut n = 0;
        for i in 0..d_len {
            if n < out.len() {
                out[n] = digits[d_len - 1 - i];
                n += 1;
            }
        }
        let suffix = b" B";
        for &b in suffix {
            if n < out.len() {
                out[n] = b;
                n += 1;
            }
        }
        n
    } else if bytes < 1024 * 1024 {
        let kb = (bytes + 512) / 1024;
        let mut digits = [0u8; 20];
        let mut d_len = 0;
        let mut val = kb;
        if val == 0 {
            digits[0] = b'0';
            d_len = 1;
        } else {
            while val > 0 && d_len < 20 {
                digits[d_len] = b'0' + (val % 10) as u8;
                val /= 10;
                d_len += 1;
            }
        }
        let mut n = 0;
        for i in 0..d_len {
            if n < out.len() {
                out[n] = digits[d_len - 1 - i];
                n += 1;
            }
        }
        let suffix = b" KB";
        for &b in suffix {
            if n < out.len() {
                out[n] = b;
                n += 1;
            }
        }
        n
    } else {
        let mut mb = bytes / (1024 * 1024);
        let mut rem_tenths = ((bytes % (1024 * 1024)) * 10 + 524288) / (1024 * 1024);
        if rem_tenths >= 10 {
            mb += 1;
            rem_tenths -= 10;
        }
        let mut digits = [0u8; 20];
        let mut d_len = 0;
        let mut val = mb;
        if val == 0 {
            digits[0] = b'0';
            d_len = 1;
        } else {
            while val > 0 && d_len < 20 {
                digits[d_len] = b'0' + (val % 10) as u8;
                val /= 10;
                d_len += 1;
            }
        }
        let mut n = 0;
        for i in 0..d_len {
            if n < out.len() {
                out[n] = digits[d_len - 1 - i];
                n += 1;
            }
        }
        if n < out.len() {
            out[n] = b'.';
            n += 1;
        }
        if n < out.len() {
            out[n] = b'0' + (rem_tenths % 10) as u8;
            n += 1;
        }
        let suffix = b" MB";
        for &b in suffix {
            if n < out.len() {
                out[n] = b;
                n += 1;
            }
        }
        n
    }
}

/// Formats resolution as ASCII string: e.g. "680x382", "1920x1080".
pub fn format_resolution(width: u32, height: u32, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if width == 0 || height == 0 {
        let na = b"Unknown";
        let n = na.len().min(out.len());
        out[..n].copy_from_slice(&na[..n]);
        return n;
    }
    let mut n = 0;
    let mut w_digits = [0u8; 10];
    let mut w_len = 0;
    let mut val = width;
    while val > 0 && w_len < 10 {
        w_digits[w_len] = b'0' + (val % 10) as u8;
        val /= 10;
        w_len += 1;
    }
    for i in 0..w_len {
        if n < out.len() {
            out[n] = w_digits[w_len - 1 - i];
            n += 1;
        }
    }
    if n < out.len() {
        out[n] = b'x';
        n += 1;
    }
    let mut h_digits = [0u8; 10];
    let mut h_len = 0;
    val = height;
    while val > 0 && h_len < 10 {
        h_digits[h_len] = b'0' + (val % 10) as u8;
        val /= 10;
        h_len += 1;
    }
    for i in 0..h_len {
        if n < out.len() {
            out[n] = h_digits[h_len - 1 - i];
            n += 1;
        }
    }
    n
}

// -----------------------------------------------------------------------------
// Alpha Blending Helper
// -----------------------------------------------------------------------------

/// Blends foreground ARGB color over background ARGB color with integer arithmetic.
/// Alpha range: 0 (100% bg) to 255 (100% fg).
#[inline(always)]
pub fn blend_argb(bg: u32, fg: u32, alpha: u32) -> u32 {
    let a = alpha.min(255);
    let inv = 255 - a;
    let r = (((bg >> 16) & 0xFF) * inv + ((fg >> 16) & 0xFF) * a) / 255;
    let g = (((bg >> 8) & 0xFF) * inv + ((fg >> 8) & 0xFF) * a) / 255;
    let b = ((bg & 0xFF) * inv + (fg & 0xFF) * a) / 255;
    0xFF000000 | (r << 16) | (g << 8) | b
}

// -----------------------------------------------------------------------------
// Playlist & Video Library Management
// -----------------------------------------------------------------------------

pub const MAX_PLAYLIST_ITEMS: usize = 32;
pub const TITLE_CAP: usize = 48;
pub const PATH_CAP: usize = 96;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlaylistItem {
    pub title: [u8; TITLE_CAP],
    pub title_len: usize,
    pub path: [u8; PATH_CAP],
    pub path_len: usize,
    pub codec: VideoCodec,
    pub file_size: u64,
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
        file_size: 0,
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

    pub fn select(&mut self, index: usize) -> Option<usize> {
        if index < self.count {
            self.current = index;
            Some(self.current)
        } else {
            None
        }
    }
}

/// Parses newline-delimited VFS entries (`<name>|<size>|<type>`) from a directory listing,
/// filtering for valid video formats, and populating `playlist`.
/// Returns the number of items added.
pub fn parse_directory_listing(
    dir_prefix: &[u8],
    fs_list_data: &[u8],
    playlist: &mut Playlist,
) -> usize {
    let mut added = 0;
    let mut start = 0;
    while start < fs_list_data.len() && playlist.count < MAX_PLAYLIST_ITEMS {
        let end = fs_list_data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| start + p)
            .unwrap_or(fs_list_data.len());
        let line = &fs_list_data[start..end];
        if !line.is_empty() {
            let mut parts = line.split(|&b| b == b'|');
            if let Some(name) = parts.next() {
                if !name.is_empty() && has_video_suffix(name) {
                    let size_bytes = parts.next();
                    let type_bytes = parts.next();
                    // If type is directory, skip
                    if type_bytes == Some(b"dir") {
                        start = end.saturating_add(1);
                        continue;
                    }
                    let mut size: u64 = 0;
                    if let Some(sb) = size_bytes {
                        for &b in sb {
                            if b.is_ascii_digit() {
                                size = size.saturating_mul(10).saturating_add((b - b'0') as u64);
                            }
                        }
                    }

                    // Build full path: dir_prefix + "/" + name
                    let mut path = [0u8; PATH_CAP];
                    let mut pl = 0;
                    let dp_len = dir_prefix.len().min(PATH_CAP);
                    path[..dp_len].copy_from_slice(&dir_prefix[..dp_len]);
                    pl += dp_len;
                    if pl > 0 && path[pl - 1] != b'/' && pl < PATH_CAP {
                        path[pl] = b'/';
                        pl += 1;
                    }
                    let nl = name.len().min(PATH_CAP.saturating_sub(pl));
                    path[pl..pl + nl].copy_from_slice(&name[..nl]);
                    pl += nl;

                    // Deduplicate against existing playlist entries
                    let already_exists = playlist.items[..playlist.count].iter().any(|item| {
                        item.path_len == pl && &item.path[..pl] == &path[..pl]
                    });

                    if !already_exists {
                        let mut item = PlaylistItem::EMPTY;
                        let tl = name.len().min(TITLE_CAP);
                        item.title[..tl].copy_from_slice(&name[..tl]);
                        item.title_len = tl;
                        item.path[..pl].copy_from_slice(&path[..pl]);
                        item.path_len = pl;
                        item.file_size = size;
                        item.codec = detect_codec_from_name(name);
                        // A directory entry carries a name and a byte size,
                        // not a frame size or a running time. Both stay zero
                        // until the file's own header is read; the screen
                        // draws an unread value as Unknown, never as a guess.

                        if playlist.add(item) {
                            added += 1;
                        }
                    }
                }
            }
        }
        start = end.saturating_add(1);
    }
    added
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
        assert_eq!(detect_codec_from_name(b"test.avi"), VideoCodec::Avi);
        assert_eq!(detect_codec_from_name(b"CLIP.AVI"), VideoCodec::Avi);
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
            255, 0, 0,     // Red
            0, 255, 0,     // Green
            0, 0, 255,     // Blue
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
        let mut ar = AspectRatio::Fit;
        assert_eq!(ar.label(), "FIT");
        ar = ar.next();
        assert_eq!(ar.label(), "FILL");
        ar = ar.next();
        assert_eq!(ar.label(), "STRETCH");
        ar = ar.next();
        assert_eq!(ar.label(), "16:9");
        ar = ar.next();
        assert_eq!(ar.label(), "4:3");
        ar = ar.next();
        assert_eq!(ar.label(), "FIT");

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
    fn test_playback_speed_selector() {
        let mut spd = PlaybackSpeed::Speed1_0x;
        assert_eq!(spd.label(), "1.0x");
        assert_eq!(spd.multiplier_milli(), 1000);

        spd = spd.next();
        assert_eq!(spd.label(), "1.25x");
        assert_eq!(spd.multiplier_milli(), 1250);

        spd = spd.next();
        assert_eq!(spd.label(), "1.5x");
        assert_eq!(spd.multiplier_milli(), 1500);

        spd = spd.next();
        assert_eq!(spd.label(), "2.0x");
        assert_eq!(spd.multiplier_milli(), 2000);

        spd = spd.next();
        assert_eq!(spd.label(), "0.5x");
        assert_eq!(spd.multiplier_milli(), 500);

        spd = spd.next();
        assert_eq!(spd.label(), "0.75x");
        assert_eq!(spd.multiplier_milli(), 750);
    }

    #[test]
    fn test_orientation_toggle() {
        let mut orient = Orientation::Portrait;
        assert_eq!(orient.label(), "PORTRAIT");
        orient = orient.toggle();
        assert_eq!(orient.label(), "LANDSCAPE");
        orient = orient.toggle();
        assert_eq!(orient.label(), "PORTRAIT");
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

        // Keyframe checks: for MJPEG/PPM/RAW every frame is a keyframe
        assert!(is_keyframe(VideoCodec::Mjpeg, 0));
        assert!(is_keyframe(VideoCodec::Mjpeg, 42));
        assert!(is_keyframe(VideoCodec::Avi, 10));
        assert!(is_keyframe(VideoCodec::PpmP6, 5));
        assert!(!is_keyframe(VideoCodec::H264, 1));
        assert_eq!(scrubber_seek_keyframe(45, 120, VideoCodec::Mjpeg), 45);
        assert_eq!(scrubber_seek_keyframe(200, 120, VideoCodec::Mjpeg), 120);
    }

    #[test]
    fn test_volume_calculations() {
        let thumb_x = volume_thumb_x(100, 400, 50);
        assert_eq!(thumb_x, 300); // 100 + (50 * 400 / 100) = 300

        let pct = volume_seek_percent(300, 100, 400);
        assert_eq!(pct, 50);

        let pct_min = volume_seek_percent(50, 100, 400);
        assert_eq!(pct_min, 0);

        let pct_max = volume_seek_percent(600, 100, 400);
        assert_eq!(pct_max, 100);
    }

    #[test]
    fn test_file_size_and_resolution_formatting() {
        let mut buf = [0u8; 24];

        let n = format_file_size(512, &mut buf);
        assert_eq!(&buf[..n], b"512 B");

        let n = format_file_size(420 * 1024, &mut buf);
        assert_eq!(&buf[..n], b"420 KB");

        let n = format_file_size((12 * 1024 * 1024) / 10, &mut buf); // 1.2 MB
        assert_eq!(&buf[..n], b"1.2 MB");

        let n = format_resolution(680, 382, &mut buf);
        assert_eq!(&buf[..n], b"680x382");

        let n = format_resolution(1920, 1080, &mut buf);
        assert_eq!(&buf[..n], b"1920x1080");
    }

    #[test]
    fn test_alpha_blending() {
        let bg = 0xFF000000;
        let fg = 0xFFFFFFFF;
        let mid = blend_argb(bg, fg, 128);
        let r = (mid >> 16) & 0xFF;
        let g = (mid >> 8) & 0xFF;
        let b = mid & 0xFF;
        assert!(r >= 127 && r <= 129);
        assert!(g >= 127 && g <= 129);
        assert!(b >= 127 && b <= 129);
    }

    #[test]
    fn test_directory_listing_parser() {
        let raw_list = b"trailer.ppm|68000|file\nclip.mp4|4500000|file\nnotes.txt|120|file\nsubfolder|0|dir\n";
        let mut playlist = Playlist::new();

        let added = parse_directory_listing(b"/storage/videos", raw_list, &mut playlist);
        assert_eq!(added, 2);
        assert_eq!(playlist.count, 2);
        assert_eq!(playlist.items[0].title_str(), "trailer.ppm");
        assert_eq!(playlist.items[0].path_str(), "/storage/videos/trailer.ppm");
        assert_eq!(playlist.items[0].file_size, 68000);
        assert_eq!(playlist.items[0].codec, VideoCodec::PpmP6);

        assert_eq!(playlist.items[1].title_str(), "clip.mp4");
        assert_eq!(playlist.items[1].path_str(), "/storage/videos/clip.mp4");
        assert_eq!(playlist.items[1].codec, VideoCodec::H264);

        // Test duplicate rejection
        let added_dup = parse_directory_listing(b"/storage/videos", raw_list, &mut playlist);
        assert_eq!(added_dup, 0);
        assert_eq!(playlist.count, 2);
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

        pl.select(0);
        assert_eq!(pl.current_item().unwrap().title_str(), "Clip1");
    }

    /// The ladder's own level -- one hundred per cent -- which is the level a
    /// person who never touched Text size draws at, and the only level at which
    /// a gate may insist on the body rung.
    const LEVEL_NORMAL: u8 = std::design::font::TEXT_SCALE_NORMAL;

    /// The volume read-out's column, as `main.rs` gives it: three line boxes of
    /// the body rung at the largest level the setting can hold, which is the
    /// widest word the read-out answers (`UNKNOWN`) is asked to stand inside.
    ///
    /// WHY read from the crate root rather than spelled again: the number the
    /// runtime face sizes words against and the number the screen paints the
    /// column from are one, and a second copy here would only ever test itself.
    const VOL_READOUT_COL: usize = super::VOLUME_READOUT_COL;

    #[test]
    fn test_the_volume_readout_words_hold_their_column_at_build_time() {
        // The same claim `main.rs` makes from a `const`, so the arithmetic the
        // build is gated on is exercised by a name a person can run.
        assert!(volume_readout_words_hold_at_every_level());
        // The narrowest rung is the last one the read-out may take, so the words
        // that stand on it are the ones the column was measured for.
        assert!(readout_run_w(UNKNOWN_LEVEL, 1, LEVEL_NORMAL) <= VOL_READOUT_COL);
        assert!(
            readout_run_w(b"100%", 1, std::design::font::TEXT_SCALE_EXTRA_LARGE) <= VOL_READOUT_COL
        );
    }

    #[test]
    fn test_loop_chip_label_names_the_mode_it_sets() {
        // The plate used to paint a bare `OFF`, which names no function. Each
        // chip label now carries the feature word the press acts on, and the
        // state on the end of it.
        assert_eq!(LoopMode::None.chip_label(), "LOOP OFF");
        assert_eq!(LoopMode::LoopOne.chip_label(), "LOOP ONE");
        assert_eq!(LoopMode::LoopAll.chip_label(), "LOOP ALL");

        // The state names the platform truth contract pins are untouched, so a
        // log line and the properties sheet still quote `OFF` / `ONE` / `ALL`.
        assert_eq!(LoopMode::None.label(), "OFF");
        assert_eq!(LoopMode::LoopOne.label(), "ONE");
        assert_eq!(LoopMode::LoopAll.label(), "ALL");

        // Every mode the cycle reaches has a chip label, so no press can land on
        // a plate that paints nothing.
        let mut mode = LoopMode::None;
        for _ in 0..6 {
            assert!(mode.chip_label().starts_with("LOOP "));
            assert!(mode.chip_label().len() > "LOOP ".len());
            mode = mode.next();
        }
    }

    #[test]
    fn test_a_chip_label_stands_inside_its_plate_at_every_rung() {
        // The six secondary plates are 154 columns and the row's labels are the
        // widest words any of their states can answer.
        let labels: [&[u8]; 12] = [
            b"LOOP OFF",
            b"LOOP ONE",
            b"LOOP ALL",
            b"0.5x",
            b"0.75x",
            b"1.0x",
            b"1.25x",
            b"1.5x",
            b"2.0x",
            b"STRETCH",
            b"16:9",
            b"4:3",
        ];
        // The two rungs `main.rs` steps a control's word between: the row's own
        // body rung and the caption rung under it.
        let rungs = [
            std::design::typography::BODY_SCALE,
            std::design::typography::CAPTION_SCALE,
        ];
        for level in 0..=3u8 {
            for text in labels {
                let rung = label_rung(text, true, CHIP_PLATE_ROOM, level, &rungs);
                assert!(
                    label_w(text, true, rung, level) <= CHIP_PLATE_ROOM,
                    "{:?} at level {} has no rung inside {} columns: rung {} measures {}",
                    core::str::from_utf8(text),
                    level,
                    CHIP_PLATE_ROOM,
                    rung,
                    label_w(text, true, rung, level),
                );
                // The step is only ever taken by a rung the person raised: at the
                // platform's own level every word holds the body rung, so the
                // control row keeps one type across the row for anybody who never
                // touched Text size.
                if level == LEVEL_NORMAL {
                    assert_eq!(
                        rung,
                        std::design::typography::BODY_SCALE,
                        "{:?} gave up the body rung at the platform level",
                        core::str::from_utf8(text),
                    );
                }
            }
        }
    }

    #[test]
    fn test_a_transport_word_stands_inside_its_row_at_every_rung() {
        // The words the transport band paints under its three discs. The play
        // control is asked for both of its faces, because the face it answers is
        // decided by the clip's state and not by the layout.
        let words: [&[u8]; 4] = [
            PREV_WORD,
            NEXT_WORD,
            transport_action_word(true),
            transport_action_word(false),
        ];

        // The word under the play disc has to move with the action the press
        // performs: one word for both faces would name an action the disc is not
        // answering, which is the defect this band is being fixed for.
        assert_ne!(
            transport_action_word(true),
            transport_action_word(false),
            "the play control paints one word for two different presses"
        );

        // Nothing but letters the monochrome kit carries: an unsupported
        // codepoint is drawn as a placeholder box, the same broken glyph this
        // fleet already refuses on the mute plate.
        for word in words {
            for byte in word {
                assert!(
                    byte.is_ascii_uppercase(),
                    "{:?} carries a byte the bitmap kit cannot ink",
                    core::str::from_utf8(word),
                );
            }
        }

        let rungs = [
            std::design::typography::BODY_SCALE,
            std::design::typography::CAPTION_SCALE,
        ];
        // The room each orientation measures its words against: the portrait row
        // and the landscape scrim are different widths, and each is the centre
        // distance the discs stand on less the air a plate keeps.
        for (name, room) in [
            ("portrait", TRANSPORT_WORD_ROOM),
            ("landscape", LAND_TRANSPORT_WORD_ROOM),
        ] {
            for level in 0..=3u8 {
                for word in words {
                    let rung = label_rung(word, true, room, level, &rungs);
                    assert!(
                        label_w(word, true, rung, level) <= room,
                        "the {} row has no rung inside {} columns for {:?} at level {}: \
                         rung {} measures {}",
                        name,
                        room,
                        core::str::from_utf8(word),
                        level,
                        rung,
                        label_w(word, true, rung, level),
                    );
                    if level == LEVEL_NORMAL {
                        assert_eq!(
                            rung,
                            std::design::typography::BODY_SCALE,
                            "{:?} gave up the body rung at the platform level on the {} row",
                            core::str::from_utf8(word),
                            name,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_still_image_formats_carry_no_running_time() {
        // A Portable Pixmap is one frame, and a RAW buffer is one frame. They are
        // in the library because the library is the filesystem, not because this
        // player can run them.
        assert!(VideoCodec::PpmP6.is_still_image());
        assert!(VideoCodec::RawRgb.is_still_image());
        assert!(VideoCodec::RawRgba.is_still_image());

        // Time-based media keeps claiming a clock, and an unknown format claims
        // nothing rather than being called a still it was never told about.
        assert!(!VideoCodec::Avi.is_still_image());
        assert!(!VideoCodec::Mjpeg.is_still_image());
        assert!(!VideoCodec::H264.is_still_image());
        assert!(!VideoCodec::Unknown.is_still_image());
    }

    #[test]
    fn test_unknown_running_time_is_words_not_a_dash_cluster() {
        let mut buf = [0u8; 24];

        // A row that only has a column of its own gets the short form; the
        // scrubber's full width gets the sentence.
        let mut narrow = [0u8; 12];
        let n = duration_label(false, 0, false, &mut narrow);
        assert_eq!(&narrow[..n], NO_TIME_SHORT);
        assert_eq!(core::str::from_utf8(&narrow[..n]).unwrap(), "NO TIME");

        let s = duration_label(false, 0, true, &mut buf);
        assert_eq!(&buf[..s], NO_RUNNING_TIME);

        // Neither form is a hyphen-and-colon mark a person reads as a glyph the
        // font is missing.
        for out in [&buf[..s], &narrow[..n]] {
            assert!(!core::str::from_utf8(out).unwrap().contains('-'));
            assert!(!core::str::from_utf8(out).unwrap().contains(':'));
        }

        // A duration that was read still formats as a time, at the padded rows
        // every other clock on this screen quotes.
        let k = duration_label(true, 65, false, &mut buf);
        assert_eq!(core::str::from_utf8(&buf[..k]).unwrap(), "01:05");
        let k = duration_label(true, 65, true, &mut buf);
        assert_eq!(core::str::from_utf8(&buf[..k]).unwrap(), "-01:05");
    }

    #[test]
    fn test_volume_readout_words_are_never_a_dash() {
        let mut buf = [0u8; 16];

        // The state the device frame is in today: the system never answered the
        // volume query, so the read-out says so in a word.
        let n = volume_readout(0, false, false, &mut buf);
        assert_eq!(&buf[..n], UNKNOWN_LEVEL);
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), "UNKNOWN");

        let n = volume_readout(40, true, true, &mut buf);
        assert_eq!(&buf[..n], MUTED);

        let n = volume_readout(0, false, true, &mut buf);
        assert_eq!(&buf[..n], b"0%");
        let n = volume_readout(45, false, true, &mut buf);
        assert_eq!(&buf[..n], b"45%");
        let n = volume_readout(100, false, true, &mut buf);
        assert_eq!(&buf[..n], b"100%");

        // A level the person dragged past the ceiling is reported at the
        // ceiling, not at the wrapped remainder.
        let n = volume_readout(250, false, true, &mut buf);
        assert_eq!(&buf[..n], b"100%");

        // The column `main.rs` gives the read-out holds one of these words at
        // every level the setting can hold, at the largest rung it fits: the
        // same step the control row takes, so a raised text size costs this
        // read-out air rather than a clipped last letter.
        let rungs = [
            std::design::typography::BODY_SCALE,
            std::design::typography::CAPTION_SCALE,
        ];
        for level in 0..=3u8 {
            for text in [UNKNOWN_LEVEL, MUTED, b"100%"] {
                let rung = label_rung(text, false, VOL_READOUT_COL, level, &rungs);
                assert!(
                    label_w(text, false, rung, level) <= VOL_READOUT_COL,
                    "{:?} at level {} does not stand in the read-out column: \
                     rung {} measures {} against {}",
                    core::str::from_utf8(text),
                    level,
                    rung,
                    label_w(text, false, rung, level),
                    VOL_READOUT_COL,
                );
                if level == LEVEL_NORMAL {
                    assert_eq!(
                        rung,
                        std::design::typography::BODY_SCALE,
                        "{:?} gave up the body rung at the platform level",
                        core::str::from_utf8(text),
                    );
                }
            }
        }
    }

    #[test]
    fn test_the_speaker_glyph_is_a_horn_in_every_state() {
        // The plate used to carry a red cross and a cone poked from four
        // hand-written offsets, which on the page colour read as a box with a
        // mark in it -- the look of a glyph the face is missing. Whatever the
        // output is doing, the horn is drawn, so the plate is always the thing
        // the press on it is named after.
        for state in [SpeakerState::On, SpeakerState::Muted, SpeakerState::Unknown] {
            assert!(speaker_hit(5, 12, state), "{:?} lost the magnet", state);
            assert!(speaker_hit(14, 12, state), "{:?} lost the mouth", state);
            // A horn and not a bar: the mouth is taller than the magnet it
            // widens out of, and the magnet is not taller than the mouth.
            assert!(speaker_hit(14, 5, state), "{:?} has no flare", state);
            assert!(!speaker_hit(8, 5, state), "{:?} is a rectangle", state);

            // Nothing reaches outside the grid the box maps into, at any state.
            for gx in -8..(ICON_GRID + 8) {
                for gy in -8..(ICON_GRID + 8) {
                    if speaker_hit(gx, gy, state) {
                        assert!(
                            gx >= 0 && gx < ICON_GRID && gy >= 0 && gy < ICON_GRID,
                            "{:?} inks ({}, {}), outside the twenty-four unit grid",
                            state,
                            gx,
                            gy,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_muting_strikes_the_horn_instead_of_replacing_it() {
        let inked = |state: SpeakerState| -> usize {
            let mut n = 0;
            for gx in 0..ICON_GRID {
                for gy in 0..ICON_GRID {
                    if speaker_hit(gx, gy, state) {
                        n += 1;
                    }
                }
            }
            n
        };
        let horn = {
            let mut n = 0;
            for gx in 0..ICON_GRID {
                for gy in 0..ICON_GRID {
                    if speaker_horn_hit(gx, gy) {
                        n += 1;
                    }
                }
            }
            n
        };

        // A live output carries waves in front of the mouth, and the muted one
        // carries a strike in the same space. The two claims are each other's
        // absence: a grid point the waves ink and the strike misses, and one the
        // strike inked where no wave ever reached.
        let mut only_on = 0;
        let mut only_muted = 0;
        let mut both = 0;
        for gx in 0..ICON_GRID {
            for gy in 0..ICON_GRID {
                let a = speaker_hit(gx, gy, SpeakerState::On);
                let b = speaker_hit(gx, gy, SpeakerState::Muted);
                if a && !b {
                    only_on += 1;
                } else if b && !a {
                    only_muted += 1;
                } else if a {
                    both += 1;
                }
            }
        }
        assert!(
            only_on >= 4 && only_muted >= 4,
            "waves and strike are not two claims about the plate: {} pixels only \
             on, {} only muted",
            only_on,
            only_muted,
        );
        assert!(both > 0, "the strike stands nowhere near the waves");

        // Every state is a picture of a speaker: the horn is inked whole in all
        // three, so no plate is a bare mark on an empty box.
        for state in [SpeakerState::On, SpeakerState::Muted, SpeakerState::Unknown] {
            assert!(inked(state) >= horn, "{:?} paints less than the horn", state);
            for gx in 0..ICON_GRID {
                for gy in 0..ICON_GRID {
                    if speaker_horn_hit(gx, gy) {
                        assert!(speaker_hit(gx, gy, state), "{:?} dropped the horn", state);
                    }
                }
            }
        }

        // The two states that make a claim about the output make it in front of
        // the mouth, so neither of them is the horn alone.
        assert!(inked(SpeakerState::On) > horn);
        assert!(inked(SpeakerState::Muted) > horn);

        // The state nobody reported is the honest one: it claims neither waves
        // nor a strike, only the horn.
        for gx in 0..ICON_GRID {
            for gy in 0..ICON_GRID {
                if !speaker_horn_hit(gx, gy) {
                    assert!(!speaker_hit(gx, gy, SpeakerState::Unknown));
                }
            }
        }

        // The painter hands this the same two fields its read-out quotes, and
        // mute answers before a level nobody read.
        assert_eq!(SpeakerState::from_volume(true, false), SpeakerState::Muted);
        assert_eq!(SpeakerState::from_volume(true, true), SpeakerState::Muted);
        assert_eq!(SpeakerState::from_volume(false, false), SpeakerState::Unknown);
        assert_eq!(SpeakerState::from_volume(false, true), SpeakerState::On);
    }
}
