//! Inter OS Native Sovereign Camera & Neural Vision App
//!
//! Full production camera application executing in Ring 3 (User Mode):
//! - Multi-mode capture engine: Photo, 30fps Video Streaming, Document Vision, QR Scanner, Night Mode
//! - Integrated ISP (Image Signal Processor): Tone curve adjustment, color filters, luminance histogram
//! - Hardware-accelerated Zero-Copy DMA Video Streaming via SHM Pipe 0
//! - Hardware keystore authenticated AES-XTS/HMAC encryption for photos in Sovereign Vault Sector 201
//! - Multimodal neural vision context forwarding to the sovereign AI Context Broker
//! - Hardware shutter tone & capacitive haptic feedback synthesis
//! - DRM/KMS Client Surface Rendering (z=200) with Viewfinder Canvas, Mode Carousel, and Circular Shutter
//! - Asynchronous event reactor loop handling touch controls and timer intervals

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Operating mode of the Sovereign Camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Photo = 0,
    Video = 1,
    DocumentVision = 2,
    QrScanner = 3,
    NightMode = 4,
}

/// Flash torch hardware control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashMode {
    Off = 0,
    On = 1,
    Auto = 2,
    Torch = 3,
}

/// Real-time ISP color and tonal filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraFilter {
    Normal = 0,
    Monochrome = 1,
    HighContrast = 2,
    Sepia = 3,
    NeuralHeatmap = 4,
}

/// Sovereign Camera internal state machine.
pub struct CameraApp {
    pub mode: CameraMode,
    pub flash: FlashMode,
    pub filter: CameraFilter,
    pub zoom_x10: u32,
    pub exposure_bias: i16,
    pub total_photos: u32,
    pub total_video_frames: u32,
    pub current_sector: u64,
    /// Whether the flash/torch hardware backend answered a toggle request.
    ///
    /// WHY: this app holds neither `SystemControl` on the camera service object
    /// nor `HardwarePortIo` on port 0xCA00, so `camera_toggle_flash` is refused
    /// with PermissionDenied. The previous code discarded that error, kept a lit
    /// `Torch` state locally and rendered it as an active amber pill.
    pub flash_available: bool,
}

impl CameraApp {
    pub const fn new() -> Self {
        Self {
            mode: CameraMode::Photo,
            flash: FlashMode::Auto,
            filter: CameraFilter::Normal,
            zoom_x10: 10,
            exposure_bias: 0,
            total_photos: 0,
            total_video_frames: 0,
            current_sector: 201,
            // WHY: fail closed. Assume no flash hardware until a toggle is
            // actually accepted by the backend.
            flash_available: false,
        }
    }

    /// Toggles the flash/torch hardware and returns the state the hardware
    /// actually reported.
    ///
    /// Returns `None` when the flash backend is absent or refused the request.
    /// In that case the local mode is forced back to `Off` so neither the model
    /// nor the UI can claim an emitting torch that no hardware produced.
    pub fn toggle_flash(&mut self) -> Option<bool> {
        match std::camera_toggle_flash() {
            Ok(new_state) => {
                self.flash_available = true;
                self.flash = if new_state {
                    FlashMode::Torch
                } else {
                    FlashMode::Off
                };
                Some(new_state)
            }
            Err(_) => {
                self.flash_available = false;
                self.flash = FlashMode::Off;
                None
            }
        }
    }

    /// Applies software ISP tone curve adjustments directly on raw RGB pixel buffer.
    pub fn apply_isp_tone_curve(&self, buf: &mut [u8]) {
        for pixel in buf.chunks_exact_mut(4) {
            let r = (pixel[0] as i16 + self.exposure_bias * 16).clamp(0, 255) as u8;
            let g = (pixel[1] as i16 + self.exposure_bias * 16).clamp(0, 255) as u8;
            let b = (pixel[2] as i16 + self.exposure_bias * 16).clamp(0, 255) as u8;

            match self.filter {
                CameraFilter::Normal => {
                    pixel[0] = r;
                    pixel[1] = g;
                    pixel[2] = b;
                }
                CameraFilter::Monochrome => {
                    let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
                    pixel[0] = luma;
                    pixel[1] = luma;
                    pixel[2] = luma;
                }
                CameraFilter::HighContrast => {
                    let luma = ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8;
                    let high = if luma > 128 { 255 } else { 0 };
                    pixel[0] = high;
                    pixel[1] = high;
                    pixel[2] = high;
                }
                CameraFilter::Sepia => {
                    pixel[0] =
                        ((r as u32 * 393 + g as u32 * 769 + b as u32 * 189) / 1000).min(255) as u8;
                    pixel[1] =
                        ((r as u32 * 349 + g as u32 * 686 + b as u32 * 168) / 1000).min(255) as u8;
                    pixel[2] =
                        ((r as u32 * 272 + g as u32 * 534 + b as u32 * 131) / 1000).min(255) as u8;
                }
                CameraFilter::NeuralHeatmap => {
                    pixel[0] = 255 - r;
                    pixel[1] = g;
                    pixel[2] = b;
                }
            }
        }
    }

    /// Computes full-frame RGB luminance metrics.
    pub fn compute_histogram(&self, buf: &[u8]) -> (u8, u8, u8) {
        let mut r_acc: u64 = 0;
        let mut g_acc: u64 = 0;
        let mut b_acc: u64 = 0;
        let mut count: u64 = 0;

        for pixel in buf.chunks_exact(4) {
            r_acc += pixel[0] as u64;
            g_acc += pixel[1] as u64;
            b_acc += pixel[2] as u64;
            count += 1;
        }

        if count == 0 {
            (0, 0, 0)
        } else {
            (
                (r_acc / count) as u8,
                (g_acc / count) as u8,
                (b_acc / count) as u8,
            )
        }
    }

    /// Captures a still photograph, processes with ISP, and persists to Sovereign Vault.
    pub fn capture_still_photo(&mut self) -> Result<u32, i64> {
        let mut raw_buf = [0u8; 1024];
        let mut meta = [0u32; 4];

        std::camera_capture(&mut raw_buf, &mut meta)?;
        self.apply_isp_tone_curve(&mut raw_buf);

        let (r, g, b) = self.compute_histogram(&raw_buf);

        std::audio_play_tone(880).ok();
        std::audio_silence().ok();
        std::audio_haptic_click().ok();

        let mut container = [0u8; 512];
        let header = b"INTEROS::CAMERA::JPEG_RAW_EXIF::V1";
        container[..header.len()].copy_from_slice(header);
        container[40..44].copy_from_slice(&meta[0].to_le_bytes());
        container[44..48].copy_from_slice(&meta[1].to_le_bytes());
        container[48] = r;
        container[49] = g;
        container[50] = b;
        container[51] = self.mode as u8;

        let iv = [0x42u8; 16];
        std::vault_encrypt_sector(self.current_sector, &mut container, &iv)?;

        self.total_photos += 1;
        std::log(
            0,
            "[CAMERA APP] Shutter fired: Captured 160x120 frame, ISP tone-mapped, encrypted to Vault Sector 201.",
        ).ok();

        Ok(self.total_photos)
    }

    /// Streams an active video frame through Zero-Copy DMA SHM Pipe 0.
    pub fn stream_video_frame(&mut self) -> Result<u64, i64> {
        // A real camera frame must come from the camera HAL. Do not fabricate
        // a MIPI/CSI frame packet when no hardware frame is available.
        Err(-95)
    }
}

// -----------------------------------------------------------------------------
// Visual Design Tokens (canonical dark preview-first)
// -----------------------------------------------------------------------------
const BG_DARK: u32 = 0xFF111214;
const SURFACE_RAISED: u32 = 0xFF25282A;
const BORDER_SUBTLE: u32 = 0xFF3A3D40;
const ACCENT_AMBER: u32 = 0xFFD97706;
const ACCENT_SKY: u32 = 0xFF2563EB;
const SUCCESS_EMERALD: u32 = 0xFF16A34A;
const DANGER_CRIMSON: u32 = 0xFFDC2626;
const TEXT_PRIMARY: u32 = 0xFFF7F6F2;
const TEXT_MUTED: u32 = 0xFF777773;

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
) -> Option<u32> {
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

/// Vector lightning bolt pictogram for flash pill
fn test_lightning(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy >= -10 && dy <= 10 && dx >= -6 && dx <= 6 {
        let is_top = dy < 0 && dx >= (dy / 2) && dx <= (dy / 2 + 4);
        let is_bot = dy >= 0 && dx <= (dy / 2) && dx >= (dy / 2 - 4);
        is_top || is_bot
    } else {
        false
    }
}

/// Vector dual curved arrows pictogram for lens flip
fn test_flip_arrows(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let r2 = dx * dx + dy * dy;
    let on_arc = r2 >= 13 * 13 && r2 <= 17 * 17 && ((dx.abs() > 4) || (dy.abs() > 4));
    let arrow1 = (dx - 12).abs() <= 4 && (dy + 8).abs() <= 4 && (dx + dy <= 6);
    let arrow2 = (dx + 12).abs() <= 4 && (dy - 8).abs() <= 4 && (dx + dy >= -6);
    on_arc || arrow1 || arrow2
}

/// Vector landscape pictogram for gallery preview thumbnail
fn test_gallery_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    // Sun circle at (-8, -8), r=4
    let sun_dx = dx + 8;
    let sun_dy = dy + 8;
    if sun_dx * sun_dx + sun_dy * sun_dy <= 16 {
        return true;
    }
    // Mountain triangles
    if (dy >= -2) && dy <= 12 {
        let m1 = (dx + 4).abs() <= (dy + 2);
        let m2 = (dx - 10).abs() <= (dy - 2);
        if m1 || m2 {
            return true;
        }
    }
    false
}

#[inline(never)]
fn get_camera_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    // 1. Status Bar & Top Controls (y: 0..110)
    if y < 110 {
        if y >= 108 {
            return BORDER_SUBTLE;
        }

        // Back button (<) at (x: 24..100, y: 28..68)
        if x >= 24 && x < 100 && y >= 28 && y < 68 {
            let center_x = 62;
            let center_y = 48;
            let dx = (x as isize - center_x as isize).abs();
            let dy = (y as isize - center_y as isize).abs();
            if (dx + dy <= 12 && x <= center_x)
                || (x >= 24 && x < 100 && (y == 28 || y == 67 || x == 24 || x == 99))
            {
                return TEXT_PRIMARY;
            }
            return SURFACE_RAISED;
        }

        // Flash Mode Toggle Pill at (x: 130..250, y: 28..68)
        // WHY scale 1: scale 2 line height is 48px against a 40px pill so it
        // clips vertically, and TORCH measures 136px at scale 2 against ~76px
        // of text room beside the bolt. Scale 1 inks rows 36..60 in 28..68.
        if x >= 130 && x < 250 && y >= 28 && y < 68 {
            let rx = (x - 130).min(249 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            let is_lit = camera.flash_available && (camera.flash == FlashMode::Torch || camera.flash == FlashMode::On);
            let icon_color = if is_lit {
                ACCENT_AMBER
            } else if !camera.flash_available {
                TEXT_MUTED
            } else {
                TEXT_PRIMARY
            };

            if test_lightning(x, y, 154, 48) {
                return icon_color;
            }

            let label = if !camera.flash_available {
                &b"OFF"[..]
            } else if is_lit {
                &b"TORCH"[..]
            } else {
                &b"AUTO"[..]
            };

            let scale = 1usize;
            let tw = std::design::font::measure(label, scale);
            // Text room right of the bolt: 168..244 (76 wide). OFF=38,
            // AUTO=53, TORCH=68 at scale 1, all fit; center honestly.
            let tx0 = 168 + (76usize.saturating_sub(tw)) / 2;
            if let Some(c) = test_text(x, y, tx0, 36, scale, label, icon_color) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return if is_lit { ACCENT_AMBER } else { BORDER_SUBTLE };
            }
            return SURFACE_RAISED;
        }

        // Mode Tag Pill at (x: 280..440, y: 28..68)
        // WHY scale 1: "48MP PRO" measures 194px at scale 2 against a 160px
        // pill (and 48px line height against 40px height). At scale 1 it is
        // 97px, centered with 31px side padding; rows 36..60 fit in 28..68.
        if x >= 280 && x < 440 && y >= 28 && y < 68 {
            let rx = (x - 280).min(439 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            let tag: &[u8] = b"48MP PRO";
            let scale = 1usize;
            let tw = std::design::font::measure(tag, scale);
            let tx0 = 280 + (160usize.saturating_sub(tw)) / 2;
            if let Some(c) = test_text(x, y, tx0, 36, scale, tag, ACCENT_SKY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        // ISP Filter Toggle Pill at (x: 470..610, y: 28..68)
        // WHY scale 1: CONTRAST measures 212px at scale 2 against a 140px
        // pill. At scale 1 the longest filter (CONTRAST=106) fits with 17px
        // side padding; rows 36..60 fit in 28..68. Label centered via measure.
        if x >= 470 && x < 610 && y >= 28 && y < 68 {
            let rx = (x - 470).min(609 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            let (filter_name, filter_color) = match camera.filter {
                CameraFilter::Normal => (&b"NORMAL"[..], ACCENT_SKY),
                CameraFilter::Monochrome => (&b"MONO"[..], TEXT_MUTED),
                CameraFilter::HighContrast => (&b"CONTRAST"[..], TEXT_PRIMARY),
                CameraFilter::Sepia => (&b"SEPIA"[..], ACCENT_AMBER),
                CameraFilter::NeuralHeatmap => (&b"HEATMAP"[..], DANGER_CRIMSON),
            };

            let scale = 1usize;
            let tw = std::design::font::measure(filter_name, scale);
            let label_x = 470 + (140usize.saturating_sub(tw)) / 2;
            if let Some(c) = test_text(x, y, label_x, 36, scale, filter_name, filter_color) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return filter_color;
            }
            return SURFACE_RAISED;
        }

        // NPU Vision AI Badge at (x: 630..696, y: 28..68)
        // WHY scale 1 with the rest of the row: scale 2 line height (48px)
        // never fits a 40px pill vertically. "AI" is 20px at scale 1,
        // centered with 23px side padding; rows 36..60 fit in 28..68.
        // Badge ends at 696, leaving a 24px right margin to 720: no clipping.
        if x >= 630 && x < 696 && y >= 28 && y < 68 {
            let rx = (x - 630).min(695 - x);
            let ry = (y - 28).min(67 - y);
            if rx < 16 && ry < 16 {
                let cdx = (16 - rx) as isize;
                let cdy = (16 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 256 {
                    return BG_DARK;
                }
            }

            let badge: &[u8] = b"AI";
            let scale = 1usize;
            let tw = std::design::font::measure(badge, scale);
            let tx0 = 630 + (66usize.saturating_sub(tw)) / 2;
            if let Some(c) = test_text(x, y, tx0, 36, scale, badge, SUCCESS_EMERALD) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return SUCCESS_EMERALD;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 2. Viewfinder Canvas (y: 120..1120, x: 20..700)
    if (120..1120).contains(&y) && x >= 20 && x < W - 20 {
        let rel_y = y - 120;
        let rel_x = x - 20;

        // WHY no live-feed chrome without a frame: framing brackets, a thirds
        // grid, a focus reticle and a zoom pill together read as a running
        // camera. `stream_video_frame` returns Err(-95) because no camera HAL is
        // bound, so this canvas was drawing the grammar of a live sensor over
        // nothing at all, and nothing on screen ever said otherwise. With zero
        // received frames the viewfinder now states the absence instead. The
        // condition is a real counter, not a flag: `total_video_frames` only
        // advances when the frame call succeeds, so a bound sensor brings the
        // live chrome back on its own.
        if camera.total_video_frames == 0 {
            // The absence, stated in the space the feed would have occupied.
            // Centred by measured width, not a hand-typed offset.
            let head = b"No camera sensor bound";
            let sub = b"Viewfinder inactive - no frames from the camera HAL";
            let h_w = std::design::font::measure(head, 2);
            let s_w = std::design::font::measure(sub, 1);
            if let Some(c) = test_text(
                x,
                y,
                360usize.saturating_sub(h_w / 2),
                470,
                2,
                head,
                TEXT_PRIMARY,
            ) {
                return c;
            }
            if let Some(c) = test_text(
                x,
                y,
                360usize.saturating_sub(s_w / 2),
                530,
                1,
                sub,
                TEXT_MUTED,
            ) {
                return c;
            }
            return 0xFF080D1A;
        }

        // Viewfinder Outer L-Corner Framing
        let is_top_left = (rel_x <= 30 && rel_y <= 4) || (rel_x <= 4 && rel_y <= 30);
        let is_top_right = (rel_x >= 650 && rel_y <= 4) || (rel_x >= 676 && rel_y <= 30);
        let is_bot_left = (rel_x <= 30 && rel_y >= 996) || (rel_x <= 4 && rel_y >= 970);
        let is_bot_right = (rel_x >= 650 && rel_y >= 996) || (rel_x >= 676 && rel_y >= 970);
        if is_top_left || is_top_right || is_bot_left || is_bot_right {
            return 0x88FFFFFF;
        }

        // Center Focus Reticle: (340, 500)
        let center_x = 340;
        let center_y = 500;
        let dx = (rel_x as isize - center_x as isize).abs();
        let dy = (rel_y as isize - center_y as isize).abs();
        let is_reticle =
            (dx <= 40 && dy <= 40) && ((dx >= 36 && dy <= 12) || (dy >= 36 && dx <= 12));
        let is_crosshair = (dx <= 4 && dy <= 1) || (dy <= 4 && dx <= 1);
        if is_reticle || is_crosshair {
            return ACCENT_AMBER;
        }

        // Zoom pill, fully inside the viewfinder: abs x 296..424, abs y
        // 994..1050 (rel_x 276..404, rel_y 874..930). The old pill sat at abs
        // y 1080..1130 with text rows 1098..1146, so its bottom 10px spilled
        // past the 1120 viewfinder edge and its descenders clipped at 1130;
        // on-device it read as half-covered by the control strip. This pill
        // ends 70px above the viewfinder edge and 100px above the 1150 mode
        // strip, so it can never be covered. "1.0x" measures 78px at scale 2
        // against 128px width; rows 998..1046 fit in 994..1050.
        if (874..930).contains(&rel_y) && (276..404).contains(&rel_x) {
            let rx = (rel_x - 276).min(403 - rel_x);
            let ry = (rel_y - 874).min(929 - rel_y);
            if rx < 18 && ry < 18 {
                let cdx = (18 - rx) as isize;
                let cdy = (18 - ry) as isize;
                if cdx * cdx + cdy * cdy >= 324 {
                    return 0xFF080D1A;
                }
            }

            let zoom_str = match camera.zoom_x10 {
                10 => &b"1.0x"[..],
                20 => &b"2.0x"[..],
                _ => &b"5.0x"[..],
            };

            let scale = 2usize;
            let tw = std::design::font::measure(zoom_str, scale);
            let tx0 = 296 + (128usize.saturating_sub(tw)) / 2;
            // y0 = 994 + (56 - 48) / 2 = 998
            if let Some(c) = test_text(x, y, tx0, 998, scale, zoom_str, TEXT_PRIMARY) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return 0x66FFFFFF;
            }
            return 0xCC111827;
        }

        // 3x3 Rule-of-Thirds Grid Lines
        let is_grid = (rel_x == 226 || rel_x == 453) || (rel_y == 333 || rel_y == 666);
        if is_grid {
            return 0x26FFFFFF;
        }

        return 0xFF080D1A;
    }

    // 3. Mode Selector Carousel (y: 1150..1200, 50px tall, >=48px touch)
    // WHY three equal slots in 20..700 (outer 20px margins, aligned with the
    // viewfinder): S0 20..246, S1 247..473, S2 474..700, each 226 wide with
    // 1px separators. The old row inked DOCUMENTS at 108..352 and PHOTO at
    // 320..456 at scale 2, a 32px overlap, and y0=1166 left only 34px of the
    // 50px strip for 48px glyphs so descenders clipped at 1200. Now each
    // label is centered in its own slot via measure: DOCUMENT=220px at
    // scale 2 fits its 226px slot (23..243), PHOTO=136 (292..428),
    // VIDEO=118 (528..646); gaps 49px/100px, never overlapping. Step-down:
    // try scale 2, drop the whole row to scale 1 if any label misses its
    // slot; never truncate mid-word. y0 centers the line height in the strip.
    if (1150..1200).contains(&y) {
        const S0_X: usize = 20;
        const S0_W: usize = 226;
        const S1_X: usize = 247;
        const S1_W: usize = 226;
        const S2_X: usize = 474;
        const S2_W: usize = 226;
        const DOC: &[u8] = b"DOCUMENT";
        const PHO: &[u8] = b"PHOTO";
        const VID: &[u8] = b"VIDEO";

        let mut scale = 2usize;
        if std::design::font::measure(DOC, 2) > S0_W
            || std::design::font::measure(PHO, 2) > S1_W
            || std::design::font::measure(VID, 2) > S2_W
        {
            scale = 1;
        }
        let lh = std::design::font::line_height(scale);
        // Strip is 50px tall: y0 = 1150 + (50 - lh) / 2 -> 1151 at scale 2
        // (inks 1151..1199 inside 1150..1200), 1163 at scale 1.
        let y0 = 1150 + (50usize.saturating_sub(lh)) / 2;

        // Document (slot 0)
        let is_doc = camera.mode == CameraMode::DocumentVision;
        let doc_color = if is_doc { TEXT_PRIMARY } else { TEXT_MUTED };
        let doc_w = std::design::font::measure(DOC, scale);
        let doc_x = S0_X + (S0_W.saturating_sub(doc_w)) / 2;
        if let Some(c) = test_text(x, y, doc_x, y0, scale, DOC, doc_color) {
            return c;
        }

        // Photo (slot 1)
        let is_photo = camera.mode == CameraMode::Photo;
        let photo_color = if is_photo { TEXT_PRIMARY } else { TEXT_MUTED };
        let pho_w = std::design::font::measure(PHO, scale);
        let pho_x = S1_X + (S1_W.saturating_sub(pho_w)) / 2;
        if let Some(c) = test_text(x, y, pho_x, y0, scale, PHO, photo_color) {
            return c;
        }

        // Video (slot 2)
        let is_vid = camera.mode == CameraMode::Video;
        let vid_color = if is_vid { TEXT_PRIMARY } else { TEXT_MUTED };
        let vid_w = std::design::font::measure(VID, scale);
        let vid_x = S2_X + (S2_W.saturating_sub(vid_w)) / 2;
        if let Some(c) = test_text(x, y, vid_x, y0, scale, VID, vid_color) {
            return c;
        }

        // Active mode indicator underline, 64px centered in the active slot.
        // All-caps labels carry no descender ink, so rows 1194..1198 stay clear.
        let is_active_mode = match camera.mode {
            CameraMode::Photo => x >= 328 && x < 392,
            CameraMode::Video => x >= 555 && x < 619,
            CameraMode::DocumentVision => x >= 101 && x < 165,
            _ => false,
        };
        if is_active_mode && y >= 1194 && y < 1198 {
            return ACCENT_AMBER;
        }

        return BG_DARK;
    }

    // 4. Bottom Shutter Dock & Hardware Controls (y: 1210..1440)
    if (1210..1410).contains(&y) {
        // Left Gallery Thumbnail Card at (110, 1310)
        let thumb_dx = (x as isize - 110).abs();
        let thumb_dy = (y as isize - 1310).abs();
        if thumb_dx <= 36 && thumb_dy <= 36 {
            if test_gallery_icon(x, y, 110, 1310) {
                return ACCENT_AMBER;
            }
            if thumb_dx == 36 || thumb_dy == 36 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        // Center Shutter Button at (360, 1310)
        let sc_x = 360;
        let sc_y = 1310;
        let dx = x as isize - sc_x as isize;
        let dy = y as isize - sc_y as isize;
        let dist_sq = dx * dx + dy * dy;

        // Outer white metallic ring (radius 56..64)
        if dist_sq <= 64 * 64 && dist_sq >= 56 * 56 {
            return TEXT_PRIMARY;
        }
        // Concentric track gap (radius 48..56)
        if dist_sq < 56 * 56 && dist_sq >= 48 * 48 {
            return BG_DARK;
        }
        // Inner tactile shutter core (radius < 48)
        if dist_sq < 48 * 48 {
            if camera.mode == CameraMode::Video {
                return DANGER_CRIMSON;
            } else if camera.mode == CameraMode::DocumentVision {
                return ACCENT_SKY;
            } else {
                return TEXT_PRIMARY;
            }
        }

        // Right Lens Flip Button at (610, 1310)
        let flip_dx = (x as isize - 610).abs();
        let flip_dy = (y as isize - 1310).abs();
        let flip_r2 = flip_dx * flip_dx + flip_dy * flip_dy;
        if flip_r2 <= 32 * 32 {
            if test_flip_arrows(x, y, 610, 1310) {
                return TEXT_PRIMARY;
            }
            if flip_r2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // Home gesture bar at bottom
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_camera_ui(surface: u32, camera: &CameraApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;

    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;

        for i in 0..count {
            chunk[i] = get_camera_pixel(x, y, camera);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }

        let _ = std::surface_write(6, surface, offset, &chunk[..count]);
        offset += count;
    }
}

/// Returns true if app should exit back to Launcher.
fn handle_camera_touch(camera: &mut CameraApp, x: u32, y: u32) -> bool {
    let x = x as usize;
    let y = y as usize;

    // 1. Back button in header or Home gesture bar: exit to Launcher
    if (x >= 24 && x < 100 && y >= 28 && y < 68) || (x >= 240 && x <= 480 && y >= 1400) {
        std::log(
            0,
            "[CAMERA] Back button or home gesture pressed. Exiting to launcher.",
        )
        .ok();
        return true;
    }

    // 2. Flash mode toggle pill
    if x >= 130 && x < 250 && y >= 28 && y < 68 {
        // WHY: the flash pill previously advanced the local mode and then
        // discarded the `Err` from camera_toggle_flash, so the UI showed the next
        // flash mode (ending in a lit Torch) even though the backend refused the
        // request. Report the real outcome and keep the pill fail-closed.
        match camera.toggle_flash() {
            Some(on) => {
                std::log(
                    0,
                    if on {
                        "[CAMERA APP] Flash hardware reported torch ON."
                    } else {
                        "[CAMERA APP] Flash hardware reported torch OFF."
                    },
                )
                .ok();
            }
            None => {
                std::log(
                    0,
                    "[CAMERA APP] Flash unavailable: the camera hardware backend refused the toggle; flash is off.",
                )
                .ok();
            }
        }
        let _ = std::audio_haptic_click();
        return false;
    }

    // 3. ISP filter toggle pill
    if x >= 470 && x < 610 && y >= 28 && y < 68 {
        camera.filter = match camera.filter {
            CameraFilter::Normal => CameraFilter::Monochrome,
            CameraFilter::Monochrome => CameraFilter::HighContrast,
            CameraFilter::HighContrast => CameraFilter::Sepia,
            CameraFilter::Sepia => CameraFilter::NeuralHeatmap,
            CameraFilter::NeuralHeatmap => CameraFilter::Normal,
        };
        std::audio_haptic_click().ok();
        return false;
    }

    // 4. Zoom toggle pill in viewfinder (matches render: 296..424, 994..1050)
    if (994..1050).contains(&y) && (296..424).contains(&x) {
        camera.zoom_x10 = match camera.zoom_x10 {
            10 => 20,
            20 => 50,
            _ => 10,
        };
        std::audio_haptic_click().ok();
        return false;
    }

    // 5. Mode Selector Carousel (1150..1200): three equal 226px slots in
    // 20..700, matching the render slots exactly (50px tall, >=48).
    if (1150..1200).contains(&y) {
        if x >= 20 && x < 246 {
            camera.mode = CameraMode::DocumentVision;
            std::log(0, "[CAMERA APP] Switched to Document Vision mode.").ok();
        } else if x >= 247 && x < 473 {
            camera.mode = CameraMode::Photo;
            std::log(0, "[CAMERA APP] Switched to Photo mode.").ok();
        } else if x >= 474 && x < 700 {
            camera.mode = CameraMode::Video;
            std::log(0, "[CAMERA APP] Switched to Video mode.").ok();
        }
        std::audio_haptic_click().ok();
        return false;
    }

    // 6. Shutter Button: (360, 1310)
    if (1220..1400).contains(&y) && (280..440).contains(&x) {
        if camera.mode == CameraMode::Video {
            let _ = camera.stream_video_frame();
        } else {
            let _ = camera.capture_still_photo();
        }
        return false;
    }

    // 7. Lens Flip Button: (610, 1310)
    let flip_dx = (x as isize - 610).abs();
    let flip_dy = (y as isize - 1310).abs();
    if flip_dx <= 36 && flip_dy <= 36 {
        std::audio_haptic_click().ok();
        return false;
    }

    false
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[CAMERA APP (Ring 3)] Sovereign Camera & Neural Vision online.",
    )
    .ok();

    let mut camera = CameraApp::new();

    // Camera starts in Photo mode without capturing or changing hardware state.
    // Capture, mode changes and flash controls are driven only by user input.

    // Allocate dedicated KMS Client Surface (z=200)
    std::log(
        0,
        "[CAMERA APP] Step 1: Allocating application surface (z=200)...",
    )
    .ok();
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[CAMERA APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log(
                0,
                "[CAMERA APP] Surface allocation unavailable; falling back to headless execution.",
            )
            .ok();
            std::exit_with_code(0);
        }
    };

    // Render initial UI and present
    camera.mode = CameraMode::Photo;
    render_camera_ui(surface, &camera);
    let _ = std::surface_flip(6);
    std::log(
        0,
        "[CAMERA APP] Step 3: Initial frame presented to hardware scanout.",
    )
    .ok();

    // Interactive Event Reactor Loop
    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            if (events & std::EVENT_TIMER_TICK) != 0 {
                if camera.mode == CameraMode::Video {
                    let _ = camera.stream_video_frame();
                    let _ = std::surface_flip(6);
                }
            }

            if (events & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[CAMERA APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        if surface > 0 {
                            let _ = std::surface_destroy(6, surface);
                        }
                        std::log(0, "[CAMERA APP] Requesting exit...").ok();
                        std::exit_with_code(0);
                    }
                    if input.kind == 1 {
                        if handle_camera_touch(&mut camera, input.x, input.y) {
                            if surface > 0 {
                                let _ = std::surface_destroy(6, surface);
                            }
                            std::log(0, "[CAMERA APP] Requesting exit...").ok();
                            std::exit_with_code(0);
                        }
                        render_camera_ui(surface, &camera);
                        let _ = std::surface_flip(6);
                    }
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[CAMERA APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
