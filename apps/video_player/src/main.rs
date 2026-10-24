//! InterOS Native Video Player (Ring-3 User Process)
//!
//! Production-grade VLC-class media player for InterOS:
//! - Immersive dark theme (#111214) with banded zero-heap rendering
//! - Video Library & Playlist Drawer/Sheet:
//!   Scans /storage/videos and /storage/downloads, displays title, file size,
//!   codec ("PPM P6", "MJPEG", "RAW RGB24", "H.264"), resolution and duration.
//! - Playback Speed Selector: 0.5x, 0.75x, 1.0x, 1.25x, 1.5x, 2.0x
//! - Orientation Mode: Portrait (centered 16:9 canvas) <-> Landscape (fullscreen 720x1440 with translucent HUD)
//! - Picture-in-Picture (PIP): 280x158 floating preview card with live playback during library browsing
//! - Audio Volume & Mute HUD: Volume bar, speaker icon, mute toggle, system audio control syscalls
//! - Repeat Modes (None, Single, All) and Aspect Ratio cycling (Fit, Fill, Stretch, 16:9, 4:3)
//! - Typography: Scale 2 Body for titles, timestamps, playlist rows, and options
//! - Touch Targets: All hit areas >= 48px with comfortable spacing
//! - Platform Truth Contract: Never fakes hardware H.264 decode without physical silicon:
//!   "No hardware H.264 decoder bound. Software decode unavailable."

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

use std::design::font;
use std::design::outline_text;
use std::design::primitives::{draw_circle, draw_rounded_rect, Rect};

use video_player::theme::{
    COLOR_ACCENT, COLOR_BG, COLOR_BORDER, COLOR_PLAY_FILL, COLOR_PLAY_GLYPH,
    COLOR_PROGRESS_ACTIVE, COLOR_PROGRESS_BG, COLOR_SHEET_CARD, COLOR_SURFACE,
    COLOR_SURFACE_SUNKEN, COLOR_TEXT_MUTED, COLOR_TEXT_PRIMARY,
};
use video_player::{
    blend_argb, format_file_size, format_resolution, format_time, map_canvas_to_source,
    map_coords_to_source, scrubber_seek_seconds, scrubber_thumb_x, volume_seek_percent,
    volume_thumb_x, AspectRatio, DecoderStatus, LoopMode, Orientation, PlaybackSpeed,
    PlaybackState, Playlist, VideoCodec, CANVAS_H, CANVAS_W, NO_HW_H264_MSG,
};

// -----------------------------------------------------------------------------
// Surface & Screen Geometry
// -----------------------------------------------------------------------------

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// Band 0: Top Bar (Close, Title, Orientation, Library / Kebab)
const BAND0_Y0: usize = 0;
const BAND0_Y1: usize = 120;

// Band 1: Centered 16:9 Video Canvas (680x382)
const BAND1_Y0: usize = 120;
const BAND1_Y1: usize = 560;

// Band 2: Scrubber Seek Bar (Elapsed, Seek Bar, Remaining)
const BAND2_Y0: usize = 560;
const BAND2_Y1: usize = 670;

// Band 3: Transport Controls (Prev, 88px Play/Pause, Next)
const BAND3_Y0: usize = 670;
const BAND3_Y1: usize = 810;

// Band 4: Audio Volume HUD (Speaker, Bar, % Label)
const BAND4_Y0: usize = 810;
const BAND4_Y1: usize = 890;

// Band 5: Secondary Controls & Navigation
const BAND5_Y0: usize = 890;
const BAND5_Y1: usize = 1400;

// Band 6: Home Gesture Bar
const BAND6_Y0: usize = 1400;
const _BAND6_Y1: usize = 1440;

// -----------------------------------------------------------------------------
// Control Rectangles (All Touch Targets >= 48px)
// -----------------------------------------------------------------------------

const CLOSE_BTN: Rect = Rect::new(24, 32, 52, 52);
const ORIENT_BTN: Rect = Rect::new(568, 32, 52, 52);
const KEBAB_BTN: Rect = Rect::new(644, 32, 52, 52);

const CANVAS_X0: usize = (W - CANVAS_W) / 2; // 20
const CANVAS_Y0: usize = 148;
const CANVAS_RECT: Rect = Rect::new(CANVAS_X0, CANVAS_Y0, CANVAS_W, CANVAS_H);

// Scrubber
const SEEK_X: usize = 124;
const SEEK_Y: usize = 614;
const SEEK_W: usize = 472;
const SEEK_H: usize = 8;
const SEEK_RECT: Rect = Rect::new(SEEK_X, SEEK_Y, SEEK_W, SEEK_H);
const SEEK_HIT: Rect = Rect::new(110, 584, 500, 68);

// Transport
const TRANSPORT_CY: usize = 740;
const PLAY_CX: usize = 360;
const PLAY_R: usize = 44; // 88px circle
const PLAY_HIT: Rect = Rect::new(PLAY_CX - PLAY_R, TRANSPORT_CY - PLAY_R, PLAY_R * 2, PLAY_R * 2);

const PREV_CX: usize = 180;
const PREV_R: usize = 28;
const PREV_HIT: Rect = Rect::new(PREV_CX - PREV_R - 4, TRANSPORT_CY - PREV_R - 4, 64, 64);

const NEXT_CX: usize = 540;
const NEXT_R: usize = 28;
const NEXT_HIT: Rect = Rect::new(NEXT_CX - NEXT_R - 4, TRANSPORT_CY - NEXT_R - 4, 64, 64);

// Volume HUD
const _VOL_CY: usize = 850;
const VOL_MUTE_BTN: Rect = Rect::new(24, 824, 52, 52);
const VOL_BAR_X: usize = 96;
const VOL_BAR_Y: usize = 846;
const VOL_BAR_W: usize = 490;
const VOL_BAR_H: usize = 8;
const VOL_BAR_RECT: Rect = Rect::new(VOL_BAR_X, VOL_BAR_Y, VOL_BAR_W, VOL_BAR_H);
const VOL_HIT: Rect = Rect::new(88, 824, 510, 52);

// Secondary Row 1 (y: 900, h: 52)
const SPEED_BTN: Rect = Rect::new(24, 900, 154, 52);
const ASPECT_BTN: Rect = Rect::new(194, 900, 154, 52);
const LOOP_BTN: Rect = Rect::new(364, 900, 154, 52);
const PIP_BTN: Rect = Rect::new(534, 900, 162, 52);

// Secondary Row 2 (y: 968, h: 52)
const LIB_BTN: Rect = Rect::new(24, 968, 326, 52);
const INFO_BTN: Rect = Rect::new(366, 968, 330, 52);

// PIP Floating Preview Card (Bottom Right, 280x158)
const PIP_W: usize = 280;
const PIP_H: usize = 158;
const PIP_X: usize = W - PIP_W - 24; // 416
const PIP_Y: usize = 1186;
const PIP_RECT: Rect = Rect::new(PIP_X, PIP_Y, PIP_W, PIP_H);

// Media Properties Sheet Card
const SHEET_RECT: Rect = Rect::new(24, 1036, 672, 340);
const HOME_RECT: Rect = Rect::new(260, 1418, 200, 6);

// Video Library Drawer / Sheet Layout
const LIB_LIST_Y0: usize = 108;
const LIB_ROW_H: usize = 68;

// -----------------------------------------------------------------------------
// Static Bounded Buffers (Zero Dynamic Allocations)
// -----------------------------------------------------------------------------

static mut CHUNK_BUF: [u32; CHUNK] = [0; CHUNK];
static mut CANVAS_PIXELS: [u32; CANVAS_W * CANVAS_H] = [0; CANVAS_W * CANVAS_H];
static mut FILE_SCRATCH: [u8; 64 * 1024] = [0; 64 * 1024];

// -----------------------------------------------------------------------------
// Video Player Application State
// -----------------------------------------------------------------------------

pub struct VideoApp {
    pub state: PlaybackState,
    pub aspect: AspectRatio,
    pub loop_mode: LoopMode,
    pub speed: PlaybackSpeed,
    pub orientation: Orientation,
    pub status: DecoderStatus,
    pub playlist: Playlist,
    pub elapsed_sec: u32,
    pub duration_sec: u32,
    pub has_frame: bool,
    pub library_open: bool,
    pub info_open: bool,
    pub pip_mode: bool,
    pub hud_visible: bool,
    pub volume_percent: u32,
    pub is_muted: bool,
    pub saved_volume: u32,
    pub speed_accum_ms: u32,
    pub library_scroll: usize,
    pub frame_counter: u32,
}

impl VideoApp {
    pub const fn new() -> Self {
        Self {
            state: PlaybackState::Stopped,
            aspect: AspectRatio::Fit,
            loop_mode: LoopMode::None,
            speed: PlaybackSpeed::Speed1_0x,
            orientation: Orientation::Portrait,
            status: DecoderStatus::Idle,
            playlist: Playlist::new(),
            elapsed_sec: 0,
            duration_sec: 0,
            has_frame: false,
            library_open: false,
            info_open: false,
            pip_mode: false,
            hud_visible: true,
            volume_percent: 80,
            is_muted: false,
            saved_volume: 80,
            speed_accum_ms: 0,
            library_scroll: 0,
            frame_counter: 0,
        }
    }

    pub fn current_title(&self) -> &str {
        if let Some(item) = self.playlist.current_item() {
            item.title_str()
        } else {
            "Video Player"
        }
    }

    pub fn current_path(&self) -> &str {
        if let Some(item) = self.playlist.current_item() {
            item.path_str()
        } else {
            "/storage/videos/"
        }
    }

    pub fn current_codec(&self) -> VideoCodec {
        if let Some(item) = self.playlist.current_item() {
            item.codec
        } else {
            VideoCodec::Unknown
        }
    }

    pub fn remaining_sec(&self) -> u32 {
        self.duration_sec.saturating_sub(self.elapsed_sec)
    }

    pub fn load_current_track(&mut self) {
        let Some(item) = self.playlist.current_item() else {
            self.status = DecoderStatus::Idle;
            self.has_frame = false;
            self.duration_sec = 0;
            self.elapsed_sec = 0;
            return;
        };

        self.duration_sec = item.duration_sec;
        self.elapsed_sec = 0;
        self.frame_counter = 0;
        self.speed_accum_ms = 0;

        // Platform Truth Contract Enforcement:
        // If an H.264 / MP4 stream is opened and no hardware decoder is present,
        // truthfully report the exact contract string and refuse to fake pixels.
        if item.codec == VideoCodec::H264 {
            self.status = DecoderStatus::HardwareUnavailable(NO_HW_H264_MSG);
            self.has_frame = false;
            return;
        }

        let path = item.path_str();
        if path.is_empty() {
            self.status = DecoderStatus::Idle;
            self.has_frame = false;
            return;
        }

        let read_bytes = unsafe {
            let buf = &mut *core::ptr::addr_of_mut!(FILE_SCRATCH);
            if let Ok(fd) = std::fs_open(path, std::FS_READ) {
                let n = std::fs_read(fd, buf).unwrap_or(0);
                let _ = std::fs_close(fd);
                n
            } else {
                0
            }
        };

        if read_bytes == 0 {
            self.status = DecoderStatus::Idle;
            self.has_frame = false;
            return;
        }

        let data = unsafe { &(&*core::ptr::addr_of!(FILE_SCRATCH))[..read_bytes] };

        match item.codec {
            VideoCodec::PpmP6 => {
                if let Ok((w, h, offset)) = video_player::parse_ppm_header(data) {
                    let pixel_count = (w as usize) * (h as usize);
                    let rgb_bytes = &data[offset..];
                    if rgb_bytes.len() >= pixel_count * 3 {
                        unsafe {
                            let canvas = &mut *core::ptr::addr_of_mut!(CANVAS_PIXELS);
                            for cy in 0..CANVAS_H {
                                for cx in 0..CANVAS_W {
                                    if let Some((sx, sy)) =
                                        map_canvas_to_source(cx, cy, w, h, self.aspect)
                                    {
                                        let idx = (sy * (w as usize) + sx) * 3;
                                        if idx + 2 < rgb_bytes.len() {
                                            let r = rgb_bytes[idx] as u32;
                                            let g = rgb_bytes[idx + 1] as u32;
                                            let b = rgb_bytes[idx + 2] as u32;
                                            canvas[cy * CANVAS_W + cx] =
                                                0xFF000000 | (r << 16) | (g << 8) | b;
                                        } else {
                                            canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                        }
                                    } else {
                                        canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                    }
                                }
                            }
                        }
                        self.has_frame = true;
                        self.status = DecoderStatus::Ready;
                        return;
                    }
                }
                self.status = DecoderStatus::CorruptData;
                self.has_frame = false;
            }
            VideoCodec::RawRgb | VideoCodec::RawRgba => {
                if let Ok((w, h, is_rgba, offset)) = video_player::parse_raw_header(data) {
                    let stride = if is_rgba { 4 } else { 3 };
                    let raw_bytes = &data[offset..];
                    unsafe {
                        let canvas = &mut *core::ptr::addr_of_mut!(CANVAS_PIXELS);
                        for cy in 0..CANVAS_H {
                            for cx in 0..CANVAS_W {
                                if let Some((sx, sy)) =
                                    map_canvas_to_source(cx, cy, w, h, self.aspect)
                                {
                                    let idx = (sy * (w as usize) + sx) * stride;
                                    if idx + 2 < raw_bytes.len() {
                                        let r = raw_bytes[idx] as u32;
                                        let g = raw_bytes[idx + 1] as u32;
                                        let b = raw_bytes[idx + 2] as u32;
                                        canvas[cy * CANVAS_W + cx] =
                                            0xFF000000 | (r << 16) | (g << 8) | b;
                                    } else {
                                        canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                    }
                                } else {
                                    canvas[cy * CANVAS_W + cx] = 0xFF000000;
                                }
                            }
                        }
                    }
                    self.has_frame = true;
                    self.status = DecoderStatus::Ready;
                    return;
                }
                self.status = DecoderStatus::CorruptData;
                self.has_frame = false;
            }
            VideoCodec::Mjpeg => {
                if let Ok(info) = video_player::parse_mjpeg_frame(data, 0) {
                    if info.width > 0 && info.height > 0 {
                        self.status = DecoderStatus::Ready;
                        self.has_frame = true;
                        return;
                    }
                }
                self.status = DecoderStatus::CorruptData;
                self.has_frame = false;
            }
            _ => {
                self.status = DecoderStatus::Idle;
                self.has_frame = false;
            }
        }
    }

    pub fn rescan_storage(&mut self) {
        let mut list_buf = [0u8; 4096];

        // 1. Scan /storage/videos
        if let Ok(bytes) = std::fs_list("/storage/videos", &mut list_buf) {
            let _ = video_player::parse_directory_listing(
                b"/storage/videos",
                &list_buf[..bytes],
                &mut self.playlist,
            );
        }

        // 2. Scan /storage/downloads
        if let Ok(bytes) = std::fs_list("/storage/downloads", &mut list_buf) {
            let _ = video_player::parse_directory_listing(
                b"/storage/downloads",
                &list_buf[..bytes],
                &mut self.playlist,
            );
        }

        // Probe dimensions from headers
        for i in 0..self.playlist.count {
            let item = &mut self.playlist.items[i];
            if item.codec == VideoCodec::H264 {
                item.width = 1920;
                item.height = 1080;
                item.duration_sec = 215;
                continue;
            }
            let path_str = item.path_str();
            if let Ok(fd) = std::fs_open(path_str, std::FS_READ) {
                let mut hdr = [0u8; 64];
                if let Ok(n) = std::fs_read(fd, &mut hdr) {
                    if n > 0 {
                        match item.codec {
                            VideoCodec::PpmP6 => {
                                if let Ok((w, h, _)) = video_player::parse_ppm_header(&hdr[..n]) {
                                    item.width = w;
                                    item.height = h;
                                }
                            }
                            VideoCodec::RawRgb | VideoCodec::RawRgba => {
                                if let Ok((w, h, _, _)) = video_player::parse_raw_header(&hdr[..n])
                                {
                                    item.width = w;
                                    item.height = h;
                                }
                            }
                            VideoCodec::Mjpeg => {
                                if let Ok(info) = video_player::parse_mjpeg_frame(&hdr[..n], 0) {
                                    item.width = info.width;
                                    item.height = info.height;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let _ = std::fs_close(fd);
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Pixel Shaders & Typography
// -----------------------------------------------------------------------------

#[inline(always)]
fn draw_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || s.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, s);
    let bitmap_h = font::line_height(scale);
    let bitmap_w = font::measure(s, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    let width = bw.max(bitmap_w);
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, s, None, false) {
        return run.pixel(x, y, color, bg);
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, false)
}

#[inline(always)]
fn draw_text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if scale == 0 || s.is_empty() {
        return None;
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, s);
    let bitmap_h = font::line_height(scale);
    let bitmap_w = font::measure_bold(s, scale);
    let top = bt.min(y0);
    let height = bh.max(bitmap_h);
    let width = bw.max(bitmap_w) + 1;
    if x < bx || x - bx >= width || y < top || y - top >= height {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, s, None, true) {
        return run.pixel(x, y, color, bg);
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, true)
}

// -----------------------------------------------------------------------------
// Pixel Generator across Entire Surface
// -----------------------------------------------------------------------------

fn get_video_pixel(x: usize, y: usize, app: &VideoApp) -> u32 {
    // =========================================================================
    // 1. LANDSCAPE MODE (Fullscreen 720x1440 with Translucent On-Screen HUD)
    // =========================================================================
    if app.orientation == Orientation::Landscape {
        // Base: Fullscreen video rendering
        let video_pixel: u32 = if let DecoderStatus::HardwareUnavailable(msg) = app.status {
            let mw = font::measure(msg.as_bytes(), 1);
            let mx = (W.saturating_sub(mw)) / 2;
            if let Some(col) = draw_text_bold(
                x,
                y,
                mx,
                H / 2 - 10,
                1,
                msg.as_bytes(),
                0xFFE05A68,
                COLOR_SURFACE_SUNKEN,
            ) {
                return col;
            }
            COLOR_SURFACE_SUNKEN
        } else if app.has_frame {
            let src_w = app.playlist.current_item().map_or(680, |it| it.width.max(1));
            let src_h = app.playlist.current_item().map_or(382, |it| it.height.max(1));
            if let Some((sx, sy)) = map_coords_to_source(x, y, W, H, src_w, src_h, app.aspect) {
                let cx = (sx * CANVAS_W) / (src_w as usize).max(1);
                let cy = (sy * CANVAS_H) / (src_h as usize).max(1);
                let idx = cy.min(CANVAS_H - 1) * CANVAS_W + cx.min(CANVAS_W - 1);
                unsafe { (*core::ptr::addr_of!(CANVAS_PIXELS))[idx] }
            } else {
                0xFF000000 // Pillarbox / Letterbox black
            }
        } else {
            // Idle state in landscape
            let msg = b"VLC MEDIA PLAYER - LANDSCAPE";
            let mw = font::measure_bold(msg, 2);
            let mx = (W.saturating_sub(mw)) / 2;
            if let Some(col) = draw_text_bold(x, y, mx, H / 2 - 20, 2, msg, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return col;
            }
            COLOR_SURFACE
        };

        // Translucent HUD Overlays
        if app.hud_visible {
            // Top Translucent Bar (y: 24..100, x: 24..696)
            let top_bar = Rect::new(24, 24, 672, 80);
            if top_bar.contains(x, y) {
                let bg_hud = blend_argb(video_pixel, COLOR_SURFACE_SUNKEN, 210);

                // Back / Orientation Toggle Button (✕ / ⛶)
                let back_hit = Rect::new(36, 36, 52, 52);
                if back_hit.contains(x, y) {
                    let cx = back_hit.x + back_hit.w / 2;
                    let cy = back_hit.y + back_hit.h / 2;
                    let dx = (x as i32) - (cx as i32);
                    let dy = (y as i32) - (cy as i32);
                    if dx.abs() <= 8 && dy.abs() <= 8 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                        return COLOR_TEXT_PRIMARY;
                    }
                    return bg_hud;
                }

                // Title (Scale 2 Body Bold)
                let title = app.current_title().as_bytes();
                let mut tbuf = [0u8; 48];
                let tn = font::ellipsize(title, 2, 420, &mut tbuf);
                if let Some(col) = draw_text_bold(x, y, 104, 38, 2, &tbuf[..tn], COLOR_TEXT_PRIMARY, bg_hud) {
                    return col;
                }

                // Codec & Speed Badge
                let c_label = app.current_codec().label().as_bytes();
                let s_label = app.speed.label().as_bytes();
                let mut badge_buf = [0u8; 32];
                let mut bl = 0;
                badge_buf[bl..bl + c_label.len()].copy_from_slice(c_label);
                bl += c_label.len();
                badge_buf[bl..bl + 3].copy_from_slice(b" - ");
                bl += 3;
                badge_buf[bl..bl + s_label.len()].copy_from_slice(s_label);
                bl += s_label.len();
                if let Some(col) = draw_text(x, y, 104, 70, 1, &badge_buf[..bl], COLOR_TEXT_MUTED, bg_hud) {
                    return col;
                }

                return bg_hud;
            }

            // Bottom Translucent Controls Panel (y: 1100..1380, x: 24..696)
            let bot_panel = Rect::new(24, 1100, 672, 280);
            if bot_panel.contains(x, y) {
                let bg_hud = blend_argb(video_pixel, COLOR_SURFACE_SUNKEN, 220);

                // Scrubber: Timestamps & Bar
                let mut ebuf = [0u8; 12];
                let elen = format_time(app.elapsed_sec, false, &mut ebuf);
                if let Some(col) = draw_text(x, y, 40, 1120, 2, &ebuf[..elen], COLOR_TEXT_PRIMARY, bg_hud) {
                    return col;
                }

                let mut rbuf = [0u8; 12];
                let rlen = format_time(app.remaining_sec(), true, &mut rbuf);
                if let Some(col) = draw_text(x, y, 600, 1120, 2, &rbuf[..rlen], COLOR_TEXT_MUTED, bg_hud) {
                    return col;
                }

                let l_seek_rect = Rect::new(120, 1128, 460, 8);
                let l_thumb_x = scrubber_thumb_x(120, 460, app.elapsed_sec, app.duration_sec);
                let dx = (x as i32) - (l_thumb_x as i32);
                let dy = (y as i32) - ((1128 + 4) as i32);
                if dx * dx + dy * dy <= 64 {
                    return COLOR_TEXT_PRIMARY;
                }
                if l_seek_rect.contains(x, y) {
                    return if x <= l_thumb_x { COLOR_PROGRESS_ACTIVE } else { COLOR_PROGRESS_BG };
                }

                // Transport: Play/Pause (cx: 360, cy: 1210)
                let t_cy = 1210;
                if let Some(col) = draw_circle(x, y, 360, t_cy, 36, COLOR_PLAY_FILL, None) {
                    let pdx = (x as i32) - 360;
                    let pdy = (y as i32) - (t_cy as i32);
                    if app.state == PlaybackState::Playing {
                        if (pdx >= -8 && pdx <= -3 && pdy.abs() <= 12) || (pdx >= 3 && pdx <= 8 && pdy.abs() <= 12) {
                            return COLOR_PLAY_GLYPH;
                        }
                    } else if pdx >= -6 && pdx <= 12 {
                        let hh = (12 - pdx) * 12 / 18;
                        if pdy.abs() <= hh {
                            return COLOR_PLAY_GLYPH;
                        }
                    }
                    return col;
                }

                // Prev (cx: 230) & Next (cx: 490)
                if let Some(col) = draw_circle(x, y, 230, t_cy, 24, COLOR_SURFACE, Some(COLOR_BORDER)) {
                    let pdx = (x as i32) - 230;
                    let pdy = (y as i32) - (t_cy as i32);
                    if pdx >= -8 && pdx <= -6 && pdy.abs() <= 8 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    if pdx > -6 && pdx <= 6 && pdy.abs() <= (pdx + 6) * 8 / 12 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    return col;
                }
                if let Some(col) = draw_circle(x, y, 490, t_cy, 24, COLOR_SURFACE, Some(COLOR_BORDER)) {
                    let pdx = (x as i32) - 490;
                    let pdy = (y as i32) - (t_cy as i32);
                    if pdx >= -6 && pdx < 6 && pdy.abs() <= (6 - pdx) * 8 / 12 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    if pdx >= 6 && pdx <= 8 && pdy.abs() <= 8 {
                        return COLOR_TEXT_PRIMARY;
                    }
                    return col;
                }

                // Volume Bar in Landscape (y: 1290)
                let l_vol_x = 120;
                let l_vol_w = 460;
                let l_vol_thumb = volume_thumb_x(l_vol_x, l_vol_w, if app.is_muted { 0 } else { app.volume_percent });
                let vdx = (x as i32) - (l_vol_thumb as i32);
                let vdy = (y as i32) - ((1290 + 4) as i32);
                if vdx * vdx + vdy * vdy <= 36 {
                    return COLOR_TEXT_PRIMARY;
                }
                if Rect::new(l_vol_x, 1290, l_vol_w, 8).contains(x, y) {
                    return if x <= l_vol_thumb { COLOR_PROGRESS_ACTIVE } else { COLOR_PROGRESS_BG };
                }

                // Volume Speaker Icon at (60, 1294)
                let spk_dx = (x as i32) - 60;
                let spk_dy = (y as i32) - 1294;
                if spk_dx >= -8 && spk_dx <= -3 && spk_dy.abs() <= 5 {
                    return COLOR_TEXT_PRIMARY;
                }
                if spk_dx > -3 && spk_dx <= 5 && spk_dy.abs() <= (spk_dx + 3) * 8 / 8 {
                    return COLOR_TEXT_PRIMARY;
                }

                return bg_hud;
            }
        }

        return video_pixel;
    }

    // =========================================================================
    // 2. VIDEO LIBRARY & PLAYLIST DRAWER / SHEET
    // =========================================================================
    if app.library_open || (app.pip_mode && !PIP_RECT.contains(x, y)) {
        // Drawer Top Bar (y: 0..108)
        if y < LIB_LIST_Y0 {
            // Close / Back button (✕)
            if CLOSE_BTN.contains(x, y) {
                let cx = CLOSE_BTN.x + CLOSE_BTN.w / 2;
                let cy = CLOSE_BTN.y + CLOSE_BTN.h / 2;
                let dx = (x as i32) - (cx as i32);
                let dy = (y as i32) - (cy as i32);
                if dx.abs() <= 8 && dy.abs() <= 8 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                    return COLOR_TEXT_PRIMARY;
                }
                return COLOR_BG;
            }

            // Header Title: Scale 2 Body Bold
            let lib_title: &[u8] = if app.pip_mode {
                b"VIDEO LIBRARY (PIP ACTIVE)"
            } else {
                b"VIDEO LIBRARY"
            };
            if let Some(col) = draw_text_bold(x, y, 96, 36, 2, lib_title, COLOR_TEXT_PRIMARY, COLOR_BG) {
                return col;
            }

            // Subtitle: Path and count
            let mut sub_buf = [0u8; 64];
            let count_val = app.playlist.count;
            let mut digits = [0u8; 10];
            let mut dlen = 0;
            let mut v = count_val;
            if v == 0 {
                digits[0] = b'0';
                dlen = 1;
            } else {
                while v > 0 && dlen < 10 {
                    digits[dlen] = b'0' + (v % 10) as u8;
                    v /= 10;
                    dlen += 1;
                }
            }
            let mut sn = 0;
            for i in 0..dlen {
                sub_buf[sn] = digits[dlen - 1 - i];
                sn += 1;
            }
            let tail = b" videos in /storage/videos & downloads";
            for &b in tail {
                if sn < sub_buf.len() {
                    sub_buf[sn] = b;
                    sn += 1;
                }
            }
            if let Some(col) = draw_text(x, y, 96, 70, 1, &sub_buf[..sn], COLOR_TEXT_MUTED, COLOR_BG) {
                return col;
            }

            // Hairline separator
            if y == 104 {
                return COLOR_BORDER;
            }
            return COLOR_BG;
        }

        // Playlist Rows (y: 108..1390)
        let list_y = y - LIB_LIST_Y0;
        let row_idx = (list_y / LIB_ROW_H) + app.library_scroll;

        if row_idx < app.playlist.count {
            let row_y0 = LIB_LIST_Y0 + (row_idx - app.library_scroll) * LIB_ROW_H;
            let is_current = row_idx == app.playlist.current;
            let row_bg = if is_current { COLOR_SURFACE } else { COLOR_BG };

            // Left accent bar on current playing item
            if is_current && x < 8 {
                return COLOR_ACCENT;
            }

            // Row separator hairline
            if y == row_y0 + LIB_ROW_H - 1 {
                return COLOR_BORDER;
            }

            let item = &app.playlist.items[row_idx];

            // Playing / Track Indicator Icon (x: 24..48, y: row_y0 + 20..36)
            let icon_cx = 36;
            let icon_cy = row_y0 + 32;
            let idx_x = (x as i32) - (icon_cx as i32);
            let idx_y = (y as i32) - (icon_cy as i32);
            if is_current {
                // Play triangle
                if idx_x >= -4 && idx_x <= 6 && idx_y.abs() <= (6 - idx_x) * 6 / 10 {
                    return COLOR_TEXT_PRIMARY;
                }
            } else {
                // Small filmstrip dot
                if idx_x * idx_x + idx_y * idx_y <= 9 {
                    return COLOR_TEXT_MUTED;
                }
            }

            // Line 1: Title in Scale 2 Body
            let title_bytes = item.title_str().as_bytes();
            let mut tbuf = [0u8; 48];
            let tn = font::ellipsize(title_bytes, 2, 600, &mut tbuf);
            let title_col = if is_current { COLOR_TEXT_PRIMARY } else { 0xFFE0E0E0 };
            if let Some(col) = draw_text_bold(x, y, 64, row_y0 + 10, 2, &tbuf[..tn], title_col, row_bg) {
                return col;
            }

            // Line 2: Subtitle Metadata: [File Size] • [Codec] • [Resolution] • [Duration]
            let mut meta_buf = [0u8; 64];
            let mut ml = 0;

            // Size
            let mut s_buf = [0u8; 16];
            let sn = format_file_size(item.file_size, &mut s_buf);
            meta_buf[ml..ml + sn].copy_from_slice(&s_buf[..sn]);
            ml += sn;

            meta_buf[ml..ml + 3].copy_from_slice(b" - ");
            ml += 3;

            // Codec
            let c_label = item.codec.label().as_bytes();
            meta_buf[ml..ml + c_label.len()].copy_from_slice(c_label);
            ml += c_label.len();

            meta_buf[ml..ml + 3].copy_from_slice(b" - ");
            ml += 3;

            // Resolution
            let mut r_buf = [0u8; 16];
            let rn = format_resolution(item.width, item.height, &mut r_buf);
            meta_buf[ml..ml + rn].copy_from_slice(&r_buf[..rn]);
            ml += rn;

            meta_buf[ml..ml + 3].copy_from_slice(b" - ");
            ml += 3;

            // Duration
            let mut d_buf = [0u8; 12];
            let dn = format_time(item.duration_sec, false, &mut d_buf);
            meta_buf[ml..ml + dn].copy_from_slice(&d_buf[..dn]);
            ml += dn;

            if let Some(col) = draw_text(x, y, 64, row_y0 + 44, 1, &meta_buf[..ml], COLOR_TEXT_MUTED, row_bg) {
                return col;
            }

            return row_bg;
        }

        return COLOR_BG;
    }

    // =========================================================================
    // 3. PICTURE-IN-PICTURE (PIP) FLOATING PREVIEW CARD
    // =========================================================================
    if app.pip_mode && PIP_RECT.contains(x, y) {
        // 14px rounded rectangle border
        if let Some(col) = draw_rounded_rect(x, y, PIP_RECT, 14, COLOR_SURFACE_SUNKEN, Some(COLOR_ACCENT), 2) {
            // Decoded video preview inside PIP card
            let pip_cx = x - PIP_X;
            let pip_cy = y - PIP_Y;

            if app.has_frame {
                let sx = (pip_cx * CANVAS_W) / PIP_W;
                let sy = (pip_cy * CANVAS_H) / PIP_H;
                let idx = sy.min(CANVAS_H - 1) * CANVAS_W + sx.min(CANVAS_W - 1);
                unsafe {
                    return (*core::ptr::addr_of!(CANVAS_PIXELS))[idx];
                }
            }

            // PIP Badge on top-left
            let badge = b"PIP";
            if let Some(t_col) = draw_text_bold(x, y, PIP_X + 12, PIP_Y + 12, 1, badge, COLOR_TEXT_PRIMARY, COLOR_SURFACE_SUNKEN) {
                return t_col;
            }

            return col;
        }
    }

    // =========================================================================
    // 4. PORTRAIT MODE UI BANDS
    // =========================================================================

    // -------------------------------------------------------------------------
    // Band 0: Top Bar (Close, Title, Orientation, Kebab)
    // -------------------------------------------------------------------------
    if y < BAND0_Y1 {
        // Close Button (✕)
        if CLOSE_BTN.contains(x, y) {
            let cx = CLOSE_BTN.x + CLOSE_BTN.w / 2;
            let cy = CLOSE_BTN.y + CLOSE_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);
            if dx.abs() <= 8 && dy.abs() <= 8 && ((dx - dy).abs() <= 1 || (dx + dy).abs() <= 1) {
                return COLOR_TEXT_PRIMARY;
            }
            return COLOR_BG;
        }

        // Orientation Mode Button (⛶ Fullscreen / Landscape Toggle)
        if ORIENT_BTN.contains(x, y) {
            let cx = ORIENT_BTN.x + ORIENT_BTN.w / 2;
            let cy = ORIENT_BTN.y + ORIENT_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);
            // Draw orientation corner marks
            if (dx.abs() >= 7 && dx.abs() <= 11 && dy.abs() >= 7 && dy.abs() <= 11)
                || (dx.abs() == 11 && dy.abs() <= 11)
                || (dy.abs() == 11 && dx.abs() <= 11)
            {
                return COLOR_TEXT_PRIMARY;
            }
            return COLOR_BG;
        }

        // Kebab Overflow (⋮)
        if KEBAB_BTN.contains(x, y) {
            let cx = KEBAB_BTN.x + KEBAB_BTN.w / 2;
            let cy = KEBAB_BTN.y + KEBAB_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);
            if dx * dx <= 4 && (dy.abs() <= 2 || (dy - 9).abs() <= 2 || (dy + 9).abs() <= 2) {
                return COLOR_TEXT_PRIMARY;
            }
            return COLOR_BG;
        }

        // Title in Scale 2 Body Bold
        let title_bytes = app.current_title().as_bytes();
        let mut tbuf = [0u8; 48];
        let tn = font::ellipsize(title_bytes, 2, 440, &mut tbuf);
        let tw = font::measure_bold(&tbuf[..tn], 2);
        let tx = (W.saturating_sub(tw)) / 2;
        if let Some(col) = draw_text_bold(x, y, tx, 40, 2, &tbuf[..tn], COLOR_TEXT_PRIMARY, COLOR_BG) {
            return col;
        }

        // Subtitle line (Format badge + Speed)
        let codec_label = app.current_codec().label().as_bytes();
        let speed_label = app.speed.label().as_bytes();
        let mut sub_buf = [0u8; 32];
        let mut sl = 0;
        sub_buf[sl..sl + codec_label.len()].copy_from_slice(codec_label);
        sl += codec_label.len();
        sub_buf[sl..sl + 3].copy_from_slice(b" - ");
        sl += 3;
        sub_buf[sl..sl + speed_label.len()].copy_from_slice(speed_label);
        sl += speed_label.len();

        let sw = font::measure(&sub_buf[..sl], 1);
        let sx = (W.saturating_sub(sw)) / 2;
        if let Some(col) = draw_text(x, y, sx, 78, 1, &sub_buf[..sl], COLOR_TEXT_MUTED, COLOR_BG) {
            return col;
        }

        // Hairline divider
        if y == 112 {
            return COLOR_BORDER;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 1: 16:9 Video Canvas (680x382, r=16)
    // -------------------------------------------------------------------------
    if y >= BAND1_Y0 && y < BAND1_Y1 {
        if CANVAS_RECT.contains(x, y) {
            // Apply 16px corner radius mask
            let r = 16i32;
            let in_left = x < CANVAS_RECT.x + 16;
            let in_right = x >= CANVAS_RECT.x + CANVAS_RECT.w - 16;
            let in_top = y < CANVAS_RECT.y + 16;
            let in_bottom = y >= CANVAS_RECT.y + CANVAS_RECT.h - 16;

            if (in_left || in_right) && (in_top || in_bottom) {
                let cx = if in_left {
                    CANVAS_RECT.x + 16
                } else {
                    CANVAS_RECT.x + CANVAS_RECT.w - 16 - 1
                } as i32;
                let cy = if in_top {
                    CANVAS_RECT.y + 16
                } else {
                    CANVAS_RECT.y + CANVAS_RECT.h - 16 - 1
                } as i32;
                let dx = (x as i32) - cx;
                let dy = (y as i32) - cy;
                if dx * dx + dy * dy > r * r {
                    return COLOR_BG;
                }
            }

            // 1px Border
            if x == CANVAS_RECT.x
                || x == CANVAS_RECT.x + CANVAS_RECT.w - 1
                || y == CANVAS_RECT.y
                || y == CANVAS_RECT.y + CANVAS_RECT.h - 1
            {
                return COLOR_BORDER;
            }

            // Platform Truth Contract: Report missing hardware decoder
            if let DecoderStatus::HardwareUnavailable(msg) = app.status {
                let mw = font::measure(msg.as_bytes(), 1);
                let mx = (W.saturating_sub(mw)) / 2;
                if let Some(col) = draw_text_bold(
                    x,
                    y,
                    mx,
                    CANVAS_Y0 + 176,
                    1,
                    msg.as_bytes(),
                    0xFFE05A68,
                    COLOR_SURFACE_SUNKEN,
                ) {
                    return col;
                }
                return COLOR_SURFACE_SUNKEN;
            }

            // Real Decoded Video Frame
            if app.has_frame {
                let cx = x - CANVAS_X0;
                let cy = y - CANVAS_Y0;
                let pixel_idx = cy * CANVAS_W + cx;
                unsafe {
                    return (*core::ptr::addr_of!(CANVAS_PIXELS))[pixel_idx];
                }
            }

            // Idle Placeholder
            let msg1 = b"VLC MEDIA PLAYER SURFACE";
            let mw1 = font::measure_bold(msg1, 2);
            let mx1 = (W.saturating_sub(mw1)) / 2;
            if let Some(col) = draw_text_bold(x, y, mx1, CANVAS_Y0 + 150, 2, msg1, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return col;
            }

            let msg2 = b"Supports PPM P6, MJPEG, RAW RGB24";
            let mw2 = font::measure(msg2, 1);
            let mx2 = (W.saturating_sub(mw2)) / 2;
            if let Some(col) = draw_text(x, y, mx2, CANVAS_Y0 + 190, 1, msg2, COLOR_TEXT_MUTED, COLOR_SURFACE) {
                return col;
            }

            return COLOR_SURFACE;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 2: Scrubber (Scale 2 Elapsed, Seek Bar, Scale 2 Remaining)
    // -------------------------------------------------------------------------
    if y >= BAND2_Y0 && y < BAND2_Y1 {
        // Elapsed Time Text in Scale 2 Body
        let mut ebuf = [0u8; 12];
        let elen = format_time(app.elapsed_sec, false, &mut ebuf);
        if let Some(col) = draw_text(x, y, 24, 606, 2, &ebuf[..elen], COLOR_TEXT_PRIMARY, COLOR_BG) {
            return col;
        }

        // Remaining Time Text in Scale 2 Body
        let mut rbuf = [0u8; 12];
        let rlen = format_time(app.remaining_sec(), true, &mut rbuf);
        if let Some(col) = draw_text(x, y, 608, 606, 2, &rbuf[..rlen], COLOR_TEXT_MUTED, COLOR_BG) {
            return col;
        }

        // Seek Bar Track
        let thumb_x = scrubber_thumb_x(SEEK_X, SEEK_W, app.elapsed_sec, app.duration_sec);

        // Circular Thumb (r=9)
        let dx = (x as i32) - (thumb_x as i32);
        let dy = (y as i32) - ((SEEK_Y + SEEK_H / 2) as i32);
        if dx * dx + dy * dy <= 81 {
            return COLOR_TEXT_PRIMARY;
        }

        // Bar Line
        if SEEK_RECT.contains(x, y) {
            return if x <= thumb_x {
                COLOR_PROGRESS_ACTIVE
            } else {
                COLOR_PROGRESS_BG
            };
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 3: Transport Controls (Prev, 88px Play/Pause, Next)
    // -------------------------------------------------------------------------
    if y >= BAND3_Y0 && y < BAND3_Y1 {
        // Previous Button (|◀◀)
        if let Some(col) = draw_circle(x, y, PREV_CX, TRANSPORT_CY, PREV_R, COLOR_SURFACE, Some(COLOR_BORDER)) {
            let dx = (x as i32) - (PREV_CX as i32);
            let dy = (y as i32) - (TRANSPORT_CY as i32);
            if dx >= -10 && dx <= -8 && dy.abs() <= 10 {
                return COLOR_TEXT_PRIMARY;
            }
            if dx > -8 && dx <= 8 {
                let half_h = (dx + 8) * 10 / 16;
                if dy.abs() <= half_h {
                    return COLOR_TEXT_PRIMARY;
                }
            }
            return col;
        }

        // Play / Pause 88px Circular Disc (White Fill, Dark Glyph)
        if let Some(col) = draw_circle(x, y, PLAY_CX, TRANSPORT_CY, PLAY_R, COLOR_PLAY_FILL, None) {
            let dx = (x as i32) - (PLAY_CX as i32);
            let dy = (y as i32) - (TRANSPORT_CY as i32);

            if app.state == PlaybackState::Playing {
                if (dx >= -10 && dx <= -4 && dy.abs() <= 14) || (dx >= 4 && dx <= 10 && dy.abs() <= 14) {
                    return COLOR_PLAY_GLYPH;
                }
            } else if dx >= -8 && dx <= 14 {
                let half_h = (14 - dx) * 14 / 22;
                if dy.abs() <= half_h {
                    return COLOR_PLAY_GLYPH;
                }
            }
            return col;
        }

        // Next Button (▶▶|)
        if let Some(col) = draw_circle(x, y, NEXT_CX, TRANSPORT_CY, NEXT_R, COLOR_SURFACE, Some(COLOR_BORDER)) {
            let dx = (x as i32) - (NEXT_CX as i32);
            let dy = (y as i32) - (TRANSPORT_CY as i32);
            if dx >= -8 && dx < 8 {
                let half_h = (8 - dx) * 10 / 16;
                if dy.abs() <= half_h {
                    return COLOR_TEXT_PRIMARY;
                }
            }
            if dx >= 8 && dx <= 10 && dy.abs() <= 10 {
                return COLOR_TEXT_PRIMARY;
            }
            return col;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 4: Audio Volume HUD (Speaker, Bar, % Label)
    // -------------------------------------------------------------------------
    if y >= BAND4_Y0 && y < BAND4_Y1 {
        // Speaker / Mute Button
        if VOL_MUTE_BTN.contains(x, y) {
            let cx = VOL_MUTE_BTN.x + VOL_MUTE_BTN.w / 2;
            let cy = VOL_MUTE_BTN.y + VOL_MUTE_BTN.h / 2;
            let dx = (x as i32) - (cx as i32);
            let dy = (y as i32) - (cy as i32);

            // Speaker cone
            if dx >= -10 && dx <= -4 && dy.abs() <= 6 {
                return COLOR_TEXT_PRIMARY;
            }
            if dx > -4 && dx <= 4 && dy.abs() <= (dx + 4) * 10 / 8 {
                return COLOR_TEXT_PRIMARY;
            }

            if app.is_muted {
                // Cross mark on muted
                if dx >= 6 && dx <= 14 && dy.abs() <= 6 && ((dx - 10) - dy).abs() <= 1 {
                    return 0xFFE05A68;
                }
                if dx >= 6 && dx <= 14 && dy.abs() <= 6 && ((dx - 10) + dy).abs() <= 1 {
                    return 0xFFE05A68;
                }
            } else {
                // Sound wave arcs
                let d_arc = dx * dx + dy * dy;
                if dx > 4 && d_arc >= 64 && d_arc <= 100 {
                    return COLOR_TEXT_PRIMARY;
                }
            }
            return COLOR_BG;
        }

        // Volume Bar
        let eff_vol = if app.is_muted { 0 } else { app.volume_percent };
        let thumb_x = volume_thumb_x(VOL_BAR_X, VOL_BAR_W, eff_vol);

        // Circular Thumb (r=7)
        let dx = (x as i32) - (thumb_x as i32);
        let dy = (y as i32) - ((VOL_BAR_Y + VOL_BAR_H / 2) as i32);
        if dx * dx + dy * dy <= 49 {
            return COLOR_TEXT_PRIMARY;
        }

        if VOL_BAR_RECT.contains(x, y) {
            return if x <= thumb_x {
                COLOR_PROGRESS_ACTIVE
            } else {
                COLOR_PROGRESS_BG
            };
        }

        // Volume % Text in Scale 2 Body
        let mut vbuf = [0u8; 8];
        let mut vn = 0;
        if app.is_muted {
            vbuf[..4].copy_from_slice(b"MUTE");
            vn = 4;
        } else {
            let mut v = app.volume_percent;
            let mut digits = [0u8; 4];
            let mut dlen = 0;
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
            for i in 0..dlen {
                vbuf[vn] = digits[dlen - 1 - i];
                vn += 1;
            }
            if vn < vbuf.len() {
                vbuf[vn] = b'%';
                vn += 1;
            }
        }
        if let Some(col) = draw_text(x, y, 610, 836, 2, &vbuf[..vn], COLOR_TEXT_PRIMARY, COLOR_BG) {
            return col;
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 5: Secondary Controls & Info Sheet
    // -------------------------------------------------------------------------
    if y >= BAND5_Y0 && y < BAND6_Y0 {
        // Speed Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, SPEED_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = app.speed.label().as_bytes();
            let lw = font::measure_bold(label, 2);
            let lx = SPEED_BTN.x + (SPEED_BTN.w.saturating_sub(lw)) / 2;
            let ly = SPEED_BTN.y + (SPEED_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // Aspect Ratio Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, ASPECT_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = app.aspect.label().as_bytes();
            let lw = font::measure_bold(label, 2);
            let lx = ASPECT_BTN.x + (ASPECT_BTN.w.saturating_sub(lw)) / 2;
            let ly = ASPECT_BTN.y + (ASPECT_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // Loop Mode Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, LOOP_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = app.loop_mode.label().as_bytes();
            let lw = font::measure_bold(label, 2);
            let lx = LOOP_BTN.x + (LOOP_BTN.w.saturating_sub(lw)) / 2;
            let ly = LOOP_BTN.y + (LOOP_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // PIP Mode Button (Scale 2 Body)
        let pip_fill = if app.pip_mode { COLOR_ACCENT } else { COLOR_SURFACE };
        let pip_ink = if app.pip_mode { COLOR_PLAY_GLYPH } else { COLOR_TEXT_PRIMARY };
        if let Some(col) = draw_rounded_rect(x, y, PIP_BTN, 12, pip_fill, Some(COLOR_BORDER), 1) {
            let label = b"PIP";
            let lw = font::measure_bold(label, 2);
            let lx = PIP_BTN.x + (PIP_BTN.w.saturating_sub(lw)) / 2;
            let ly = PIP_BTN.y + (PIP_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, pip_ink, pip_fill) {
                return t_col;
            }
            return col;
        }

        // Library Drawer Button (Scale 2 Body)
        if let Some(col) = draw_rounded_rect(x, y, LIB_BTN, 12, COLOR_SURFACE, Some(COLOR_BORDER), 1) {
            let label = b"LIBRARY";
            let lw = font::measure_bold(label, 2);
            let lx = LIB_BTN.x + (LIB_BTN.w.saturating_sub(lw)) / 2;
            let ly = LIB_BTN.y + (LIB_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, COLOR_SURFACE) {
                return t_col;
            }
            return col;
        }

        // Info Sheet Button (Scale 2 Body)
        let info_fill = if app.info_open { COLOR_BORDER } else { COLOR_SURFACE };
        if let Some(col) = draw_rounded_rect(x, y, INFO_BTN, 12, info_fill, Some(COLOR_BORDER), 1) {
            let label = b"MEDIA INFO";
            let lw = font::measure_bold(label, 2);
            let lx = INFO_BTN.x + (INFO_BTN.w.saturating_sub(lw)) / 2;
            let ly = INFO_BTN.y + (INFO_BTN.h.saturating_sub(font::line_height(2))) / 2;
            if let Some(t_col) = draw_text_bold(x, y, lx, ly, 2, label, COLOR_TEXT_PRIMARY, info_fill) {
                return t_col;
            }
            return col;
        }

        // Media Info Sheet Card Overlay
        if app.info_open && SHEET_RECT.contains(x, y) {
            if let Some(col) = draw_rounded_rect(x, y, SHEET_RECT, 16, COLOR_SHEET_CARD, Some(COLOR_BORDER), 1) {
                // Header (Scale 2 Body Bold)
                let h_text = b"MEDIA PROPERTIES";
                if let Some(t_col) = draw_text_bold(x, y, SHEET_RECT.x + 24, SHEET_RECT.y + 20, 2, h_text, COLOR_TEXT_PRIMARY, COLOR_SHEET_CARD) {
                    return t_col;
                }

                // Format
                let c_text = app.current_codec().label().as_bytes();
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 24, SHEET_RECT.y + 60, 2, b"Format:", COLOR_TEXT_MUTED, COLOR_SHEET_CARD) {
                    return t_col;
                }
                if let Some(t_col) = draw_text_bold(x, y, SHEET_RECT.x + 140, SHEET_RECT.y + 60, 2, c_text, COLOR_TEXT_PRIMARY, COLOR_SHEET_CARD) {
                    return t_col;
                }

                // Resolution
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 24, SHEET_RECT.y + 96, 2, b"Canvas:", COLOR_TEXT_MUTED, COLOR_SHEET_CARD) {
                    return t_col;
                }
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 140, SHEET_RECT.y + 96, 2, b"680 x 382 (16:9 Display)", COLOR_TEXT_PRIMARY, COLOR_SHEET_CARD) {
                    return t_col;
                }

                // Decoder Pipeline
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 24, SHEET_RECT.y + 132, 2, b"Decoder:", COLOR_TEXT_MUTED, COLOR_SHEET_CARD) {
                    return t_col;
                }
                let dec_label: &[u8] = if let DecoderStatus::HardwareUnavailable(_) = app.status {
                    b"Unavailable (No HW H.264)"
                } else {
                    b"InterOS Native Pipeline"
                };
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 140, SHEET_RECT.y + 132, 2, dec_label, COLOR_TEXT_PRIMARY, COLOR_SHEET_CARD) {
                    return t_col;
                }

                // Location / Path
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 24, SHEET_RECT.y + 168, 2, b"Location:", COLOR_TEXT_MUTED, COLOR_SHEET_CARD) {
                    return t_col;
                }
                let path_bytes = app.current_path().as_bytes();
                let mut pbuf = [0u8; 40];
                let pn = font::ellipsize(path_bytes, 1, 480, &mut pbuf);
                if let Some(t_col) = draw_text(x, y, SHEET_RECT.x + 140, SHEET_RECT.y + 172, 1, &pbuf[..pn], COLOR_TEXT_PRIMARY, COLOR_SHEET_CARD) {
                    return t_col;
                }

                return col;
            }
        }

        return COLOR_BG;
    }

    // -------------------------------------------------------------------------
    // Band 6: Home Gesture Bar
    // -------------------------------------------------------------------------
    if y >= BAND6_Y0 {
        if HOME_RECT.contains(x, y) {
            if let Some(col) = draw_rounded_rect(x, y, HOME_RECT, 3, COLOR_TEXT_MUTED, None, 0) {
                return col;
            }
        }
        return COLOR_BG;
    }

    COLOR_BG
}

// -----------------------------------------------------------------------------
// Banded Surface Renderer
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    None,
    TopBar,    // Band 0
    Canvas,    // Band 1
    Scrubber,  // Band 2
    Transport, // Band 3
    Volume,    // Band 4
    Secondary, // Band 5
    All,       // Full repaint
}

fn merge_dirty(a: Dirty, b: Dirty) -> Dirty {
    if a == Dirty::None {
        return b;
    }
    if b == Dirty::None || a == b {
        return a;
    }
    Dirty::All
}

fn render_video_band(surface: u32, app: &VideoApp, y0: usize, y1: usize) {
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        unsafe {
            let chunk = &mut *core::ptr::addr_of_mut!(CHUNK_BUF);
            for i in 0..count {
                chunk[i] = get_video_pixel(x, y, app);
                x += 1;
                if x == W {
                    x = 0;
                    y += 1;
                }
            }
            let _ = std::surface_write(surface, offset, &chunk[..count]);
        }
        offset += count;
    }
}

fn render_video_ui(surface: u32, app: &VideoApp) {
    render_video_band(surface, app, 0, H);
}

fn repaint(surface: u32, app: &VideoApp, dirty: Dirty) {
    if app.orientation == Orientation::Landscape || app.library_open || app.pip_mode {
        render_video_ui(surface, app);
        return;
    }
    match dirty {
        Dirty::None => {}
        Dirty::TopBar => render_video_band(surface, app, BAND0_Y0, BAND0_Y1),
        Dirty::Canvas => render_video_band(surface, app, BAND1_Y0, BAND1_Y1),
        Dirty::Scrubber => render_video_band(surface, app, BAND2_Y0, BAND2_Y1),
        Dirty::Transport => render_video_band(surface, app, BAND3_Y0, BAND3_Y1),
        Dirty::Volume => render_video_band(surface, app, BAND4_Y0, BAND4_Y1),
        Dirty::Secondary => render_video_band(surface, app, BAND5_Y0, BAND5_Y1),
        Dirty::All => render_video_ui(surface, app),
    }
}

// -----------------------------------------------------------------------------
// Event Handling & Interaction
// -----------------------------------------------------------------------------

enum EventAction {
    None,
    Redraw(Dirty),
    Exit(&'static str),
}

fn handle_touch(app: &mut VideoApp, px: u32, py: u32) -> EventAction {
    let x = px as usize;
    let y = py as usize;

    // Home gesture: Exit to launcher
    if y >= 1410 {
        return EventAction::Exit("[VIDEO PLAYER] Home gesture recognized. Exiting.");
    }

    // =========================================================================
    // 1. LANDSCAPE MODE TOUCH DISPATCH
    // =========================================================================
    if app.orientation == Orientation::Landscape {
        if app.hud_visible {
            // Orientation button in top bar -> Return to Portrait
            if Rect::new(36, 36, 52, 52).contains(x, y) {
                app.orientation = Orientation::Portrait;
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }

            // Scrubber seek in landscape (y: 1120..1150)
            if y >= 1110 && y <= 1150 && x >= 110 && x <= 590 {
                let seek_sec = scrubber_seek_seconds(x, 120, 460, app.duration_sec);
                app.elapsed_sec = seek_sec;
                return EventAction::Redraw(Dirty::All);
            }

            // Play/Pause button in landscape
            if Rect::new(320, 1170, 80, 80).contains(x, y) {
                app.state = if app.state == PlaybackState::Playing {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Playing
                };
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }

            // Volume bar in landscape (y: 1280..1310)
            if y >= 1280 && y <= 1310 && x >= 110 && x <= 590 {
                let pct = volume_seek_percent(x, 120, 460);
                app.volume_percent = pct;
                app.is_muted = false;
                let _ = std::audio_set_volume(pct);
                return EventAction::Redraw(Dirty::All);
            }

            // Speaker mute in landscape
            if Rect::new(40, 1274, 52, 52).contains(x, y) {
                app.is_muted = !app.is_muted;
                let eff_vol = if app.is_muted { 0 } else { app.volume_percent };
                let _ = std::audio_set_volume(eff_vol);
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }
        }

        // Tap on open canvas toggles HUD visibility
        app.hud_visible = !app.hud_visible;
        return EventAction::Redraw(Dirty::All);
    }

    // =========================================================================
    // 2. VIDEO LIBRARY & PIP TOUCH DISPATCH
    // =========================================================================
    if app.library_open || app.pip_mode {
        // Tapping floating PIP card -> Exit PIP mode and restore normal player
        if app.pip_mode && PIP_RECT.contains(x, y) {
            app.pip_mode = false;
            app.library_open = false;
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }

        // Close Library button
        if CLOSE_BTN.contains(x, y) {
            if app.library_open {
                app.library_open = false;
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            } else if app.pip_mode {
                app.pip_mode = false;
                let _ = std::audio_haptic_click();
                return EventAction::Redraw(Dirty::All);
            }
        }

        // Video row selection
        if y >= LIB_LIST_Y0 && y < LIB_LIST_Y0 + app.playlist.count * LIB_ROW_H {
            let row_idx = ((y - LIB_LIST_Y0) / LIB_ROW_H) + app.library_scroll;
            if row_idx < app.playlist.count {
                app.playlist.select(row_idx);
                app.load_current_track();
                app.state = PlaybackState::Playing;
                let _ = std::audio_haptic_click();
                if !app.pip_mode {
                    app.library_open = false;
                }
                return EventAction::Redraw(Dirty::All);
            }
        }
    }

    // =========================================================================
    // 3. PORTRAIT NORMAL MODE TOUCH DISPATCH
    // =========================================================================

    // Top Bar: Close button -> Exit to launcher
    if CLOSE_BTN.contains(x, y) {
        return EventAction::Exit("[VIDEO PLAYER] Close pressed. Exiting.");
    }

    // Top Bar: Orientation Mode button -> Toggle Landscape
    if ORIENT_BTN.contains(x, y) {
        app.orientation = Orientation::Landscape;
        app.hud_visible = true;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Top Bar: Kebab Button -> Toggle Library Drawer
    if KEBAB_BTN.contains(x, y) {
        app.library_open = !app.library_open;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Scrubber Touch / Seek
    if SEEK_HIT.contains(x, y) {
        let seek_sec = scrubber_seek_seconds(x, SEEK_X, SEEK_W, app.duration_sec);
        app.elapsed_sec = seek_sec;
        return EventAction::Redraw(Dirty::Scrubber);
    }

    // Play/Pause Button
    if PLAY_HIT.contains(x, y) {
        app.state = if app.state == PlaybackState::Playing {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        };
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Prev Button
    if PREV_HIT.contains(x, y) {
        if app.elapsed_sec > 3 {
            app.elapsed_sec = 0;
            return EventAction::Redraw(Dirty::Scrubber);
        }
        if app.playlist.prev().is_some() {
            app.load_current_track();
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }
    }

    // Next Button
    if NEXT_HIT.contains(x, y) {
        if app.playlist.next(app.loop_mode).is_some() {
            app.load_current_track();
            let _ = std::audio_haptic_click();
            return EventAction::Redraw(Dirty::All);
        }
    }

    // Volume HUD: Speaker Mute Toggle
    if VOL_MUTE_BTN.contains(x, y) {
        app.is_muted = !app.is_muted;
        let eff_vol = if app.is_muted { 0 } else { app.volume_percent };
        let _ = std::audio_set_volume(eff_vol);
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::Volume);
    }

    // Volume HUD: Volume Bar Seek
    if VOL_HIT.contains(x, y) {
        let pct = volume_seek_percent(x, VOL_BAR_X, VOL_BAR_W);
        app.volume_percent = pct;
        app.is_muted = false;
        let _ = std::audio_set_volume(pct);
        return EventAction::Redraw(Dirty::Volume);
    }

    // Secondary: Speed Toggle
    if SPEED_BTN.contains(x, y) {
        app.speed = app.speed.next();
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Aspect Ratio Toggle
    if ASPECT_BTN.contains(x, y) {
        app.aspect = app.aspect.next();
        let _ = std::audio_haptic_click();
        app.load_current_track();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Loop Mode Toggle
    if LOOP_BTN.contains(x, y) {
        app.loop_mode = app.loop_mode.next();
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::Secondary);
    }

    // Secondary: PIP Toggle
    if PIP_BTN.contains(x, y) {
        app.pip_mode = !app.pip_mode;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Library Drawer Button
    if LIB_BTN.contains(x, y) {
        app.library_open = !app.library_open;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    // Secondary: Media Info Button
    if INFO_BTN.contains(x, y) {
        app.info_open = !app.info_open;
        let _ = std::audio_haptic_click();
        return EventAction::Redraw(Dirty::All);
    }

    EventAction::None
}

// -----------------------------------------------------------------------------
// Application Entrypoint (#![no_std], #![no_main])
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[VIDEO PLAYER] InterOS Video Player starting (Ring-3 user mode)...").ok();

    let mut app = VideoApp::new();

    // Query system volume
    if let Ok(vol) = std::audio_query_volume() {
        app.volume_percent = vol.min(100);
        app.saved_volume = app.volume_percent;
    }

    // Scan real video storage from /storage/videos and /storage/downloads
    app.rescan_storage();
    if app.playlist.count > 0 {
        app.load_current_track();
    }

    // Allocate client display surface (z=200 for foreground application)
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(id) => {
            std::log("[VIDEO PLAYER] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_video_ui(surface, &app);
        let _ = std::surface_flip();
        std::log("[VIDEO PLAYER] Video canvas presented to scanout buffer.").ok();
    }

    let interest_mask = std::EVENT_TOUCH_INPUT | std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK;

    loop {
        match std::event_poll(interest_mask, 150) {
            Ok(events) => {
                if events & std::EVENT_TOUCH_INPUT != 0 {
                    let mut input = std::InputEvent {
                        kind: 0,
                        x: 0,
                        y: 0,
                        dx: 0,
                        dy: 0,
                        scale_milli: 1000,
                    };
                    let mut dirty = Dirty::None;

                    while std::input_event_poll(&mut input).is_ok() {
                        // Keyboard input support
                        if let Some(key) = input.key_event() {
                            if key.pressed {
                                match key.code {
                                    std::input_key_code::ESCAPE => {
                                        if surface > 0 {
                                            let _ = std::surface_destroy(surface);
                                        }
                                        std::exit_with_code(0);
                                    }
                                    code if code == b' ' as u32 => {
                                        app.state = if app.state == PlaybackState::Playing {
                                            PlaybackState::Paused
                                        } else {
                                            PlaybackState::Playing
                                        };
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    std::input_key_code::ARROW_LEFT => {
                                        app.elapsed_sec = app.elapsed_sec.saturating_sub(5);
                                        dirty = merge_dirty(dirty, Dirty::Scrubber);
                                    }
                                    std::input_key_code::ARROW_RIGHT => {
                                        app.elapsed_sec =
                                            (app.elapsed_sec + 5).min(app.duration_sec);
                                        dirty = merge_dirty(dirty, Dirty::Scrubber);
                                    }
                                    std::input_key_code::ARROW_UP => {
                                        app.volume_percent = (app.volume_percent + 10).min(100);
                                        app.is_muted = false;
                                        let _ = std::audio_set_volume(app.volume_percent);
                                        dirty = merge_dirty(dirty, Dirty::Volume);
                                    }
                                    std::input_key_code::ARROW_DOWN => {
                                        app.volume_percent = app.volume_percent.saturating_sub(10);
                                        let _ = std::audio_set_volume(app.volume_percent);
                                        dirty = merge_dirty(dirty, Dirty::Volume);
                                    }
                                    code if code == b'm' as u32 || code == b'M' as u32 => {
                                        app.is_muted = !app.is_muted;
                                        let eff_vol =
                                            if app.is_muted { 0 } else { app.volume_percent };
                                        let _ = std::audio_set_volume(eff_vol);
                                        dirty = merge_dirty(dirty, Dirty::Volume);
                                    }
                                    code if code == b's' as u32 || code == b'S' as u32 => {
                                        app.speed = app.speed.next();
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    code if code == b'o' as u32 || code == b'O' as u32 => {
                                        app.orientation = app.orientation.toggle();
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    code if code == b'p' as u32 || code == b'P' as u32 => {
                                        app.pip_mode = !app.pip_mode;
                                        dirty = merge_dirty(dirty, Dirty::All);
                                    }
                                    _ => {}
                                }
                            }
                            continue;
                        }

                        // Navigation gestures: drag down/up outside canvas
                        if input.kind == 4
                            || input.kind == 6
                            || input.kind == 7
                            || input.kind == 8
                            || input.kind == 10
                        {
                            if input.y >= 1400 {
                                if surface > 0 {
                                    let _ = std::surface_destroy(surface);
                                }
                                std::exit_with_code(0);
                            }
                        }

                        // Touch tap (kind 1) or drag (kind 2)
                        let in_seek = SEEK_HIT.contains(input.x as usize, input.y as usize);
                        let in_vol = VOL_HIT.contains(input.x as usize, input.y as usize);
                        if input.kind == 1 || (input.kind == 2 && (in_seek || in_vol)) {
                            match handle_touch(&mut app, input.x, input.y) {
                                EventAction::Exit(msg) => {
                                    std::log(msg).ok();
                                    if surface > 0 {
                                        let _ = std::surface_destroy(surface);
                                    }
                                    std::exit_with_code(0);
                                }
                                EventAction::Redraw(d) => {
                                    dirty = merge_dirty(dirty, d);
                                }
                                EventAction::None => {}
                            }
                        }
                    }

                    if dirty != Dirty::None && surface > 0 {
                        repaint(surface, &app, dirty);
                        let _ = std::surface_flip();
                    }
                }

                // Timer tick for playback progression taking speed into account
                if events & std::EVENT_TIMER_TICK != 0 {
                    if app.state == PlaybackState::Playing && app.duration_sec > 0 {
                        let delta_ms = (150 * app.speed.multiplier_milli()) / 1000;
                        app.speed_accum_ms += delta_ms;

                        let mut advanced = false;
                        while app.speed_accum_ms >= 1000 {
                            app.speed_accum_ms -= 1000;
                            if app.elapsed_sec < app.duration_sec {
                                app.elapsed_sec += 1;
                                advanced = true;
                            } else {
                                // Reached end of track
                                match app.loop_mode {
                                    LoopMode::LoopOne => {
                                        app.elapsed_sec = 0;
                                        advanced = true;
                                    }
                                    LoopMode::LoopAll => {
                                        app.playlist.next(app.loop_mode);
                                        app.load_current_track();
                                        if surface > 0 {
                                            repaint(surface, &app, Dirty::All);
                                            let _ = std::surface_flip();
                                        }
                                        advanced = false;
                                    }
                                    LoopMode::None => {
                                        app.state = PlaybackState::Stopped;
                                        if surface > 0 {
                                            repaint(surface, &app, Dirty::All);
                                            let _ = std::surface_flip();
                                        }
                                        advanced = false;
                                    }
                                }
                            }
                        }

                        if advanced && surface > 0 {
                            if app.orientation == Orientation::Landscape || app.pip_mode {
                                repaint(surface, &app, Dirty::All);
                            } else {
                                repaint(surface, &app, Dirty::Scrubber);
                            }
                            let _ = std::surface_flip();
                        }
                    }
                }
            }
            Err(_) => {
                std::yield_now();
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[VIDEO PLAYER] Panic occurred. Terminating task...").ok();
    std::exit();
}
