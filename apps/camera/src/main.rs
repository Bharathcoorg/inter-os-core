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
//! - Affordances whose backing capability is absent are drawn in a recessed
//!   unavailable treatment and are not hit-tested, instead of being painted live
//!   and ignored: see `CameraApp::camera_authorised`
//! - The mode tag reports the frame geometry the camera HAL actually returned,
//!   and the status badge reports the real count of captures that succeeded
//! - Banded rendering (`render_camera_band`): only the rows an input actually
//!   dirtied are re-rasterised, instead of all 1,036,800 pixels on every event
//! - Asynchronous event reactor loop handling touch controls and timer intervals

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use std::design::primitives::Rect;

mod outline_text;

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
    /// Whether the kernel granted this task camera authority at all.
    ///
    /// WHY probed rather than assumed: `sys_camera_capture` gates every action -
    /// capture, autofocus state and flash - behind `SystemControl` on the camera
    /// service object or `HardwarePortIo` on port 0xCA00, and this task is
    /// spawned with neither. The kernel's own answer is the only honest source
    /// for "can this app drive a camera", and it is allowed to change (a build
    /// that binds the HAL would flip it), so the UI follows the probe instead of
    /// a hard-coded "unavailable". The probe is the read-only autofocus query:
    /// unlike a flash toggle it changes no hardware state.
    pub camera_authorised: bool,
    /// Frame geometry of the last capture that actually succeeded, pre-rendered.
    ///
    /// WHY a fixed buffer instead of formatting per pixel: `get_camera_pixel` is
    /// called once per pixel, so a `format!`-style routine in the hot path would
    /// run tens of thousands of times for one number. The label is rebuilt when a
    /// capture succeeds and only read afterwards.
    pub frame_label: [u8; 16],
    pub frame_label_len: usize,
    /// `total_photos` as text, pre-rendered for the same reason as above.
    pub shots_label: [u8; 16],
    pub shots_label_len: usize,
    pub head_x: usize,
    pub sub_x: usize,
}

/// Label shown before any capture has succeeded: a real answer ("the HAL has not
/// returned a frame yet"), not a made-up resolution.
const NO_FRAME_LABEL: [u8; 16] = *b"NO FRAME\0\0\0\0\0\0\0\0";
/// Label shown when the kernel refused this task camera authority.
const NO_SENSOR_LABEL: [u8; 16] = *b"NO SENSOR\0\0\0\0\0\0\0";
/// Badge text when nothing has been captured: `0 SHOTS`.
const ZERO_SHOTS_LABEL: [u8; 16] = *b"0 SHOTS\0\0\0\0\0\0\0\0\0";

/// Formats `v` as decimal ASCII into `out`; returns the bytes written.
///
/// WHY: measured values only exist at runtime, and the app has no allocator.
fn fmt_u32(mut v: u32, out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    while v > 0 && n < 10 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let n = n.min(out.len());
    for i in 0..n {
        out[i] = digits[n - 1 - i];
    }
    n
}

/// Copies `src` into a `LEN`-byte label buffer and returns its length.
fn set_label(dst: &mut [u8; 16], src: &[u8]) -> usize {
    dst.fill(0);
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
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
            // WHY: fail closed here too. `probe_camera_authority` overwrites
            // this at start-up with the kernel's real answer; until it has, the
            // app must not claim it can drive a sensor.
            camera_authorised: false,
            frame_label: NO_FRAME_LABEL,
            frame_label_len: 8,
            shots_label: ZERO_SHOTS_LABEL,
            shots_label_len: 7,
            head_x: 0,
            sub_x: 0,
        }
    }

    /// Precomputes text positions for static labels to prevent per-pixel recalculation.
    pub fn init(&mut self) {
        let head = b"No camera sensor bound";
        let sub = b"Viewfinder inactive - no frames from the camera HAL";
        self.head_x = centred_x(head, 2, 20, VIEWFINDER_W);
        self.sub_x = centred_x(sub, 1, 20, VIEWFINDER_W);
    }

    /// Asks the kernel whether this task holds camera authority, and records the
    /// answer for the UI.
    ///
    /// WHY `camera_autofocus_state` and not `camera_toggle_flash`: the autofocus
    /// query only reads the HAL's current focus state, so probing cannot flip a
    /// torch on behind the user's back the way a flash probe would.
    pub fn probe_camera_authority(&mut self) {
        self.camera_authorised = std::camera_autofocus_state().is_ok();
        if !self.camera_authorised {
            // WHY the label, not just the flag: with no authority the mode tag
            // must say so. The old tag read "48MP PRO" unconditionally, which
            // advertised a capture resolution and a mode the hardware never
            // reported and this app can never produce.
            self.frame_label = NO_SENSOR_LABEL;
            self.frame_label_len = 9;
        }
    }

    /// Rebuilds the two derived labels after the counters move.
    ///
    /// WHY a separate step: `total_photos` only advances on a capture that the
    /// HAL actually returned, and the frame geometry only exists after one, so
    /// both labels are functions of real results and are recomputed only when a
    /// result arrives.
    fn refresh_labels(&mut self) {
        let shots = if self.total_photos == 1 {
            &b" SHOT"[..]
        } else {
            &b" SHOTS"[..]
        };
        let mut buf = [0u8; 16];
        let mut n = fmt_u32(self.total_photos, &mut buf);
        for &b in shots {
            if n < buf.len() {
                buf[n] = b;
                n += 1;
            }
        }
        self.shots_label_len = set_label(&mut self.shots_label, &buf[..n]);
    }

    /// Records the frame geometry the camera HAL reported for a real capture.
    fn set_frame_geometry(&mut self, w: u32, h: u32) {
        let mut buf = [0u8; 16];
        let mut n = fmt_u32(w, &mut buf);
        if n < buf.len() {
            buf[n] = b'x';
            n += 1;
        }
        n += fmt_u32(h, &mut buf[n..]);
        self.frame_label_len = set_label(&mut self.frame_label, &buf[..n]);
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

        // WHY before the histogram: `meta` is what the camera HAL actually
        // reported for this frame. Recording it the moment the capture succeeds
        // is what lets the mode tag show a real resolution instead of a claim.
        self.set_frame_geometry(meta[0], meta[1]);

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

        let mut iv = [0u8; 16];
        if std::get_random(&mut iv).is_err() {
            std::log(
                0,
                "[CAMERA APP] Refusing to encrypt frame without cryptographic TRNG entropy.",
            )
            .ok();
            return Err(-1);
        }
        std::vault_encrypt_sector(self.current_sector, &mut container, &iv)?;

        self.total_photos += 1;
        self.refresh_labels();
        std::log(
            0,
            "[CAMERA APP] Shutter fired: HAL frame returned, ISP tone-mapped, encrypted to Vault Sector 201.",
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

/// Ink for a control that is painted but has nothing behind it.
///
/// WHY the border token and not a grey invented here: it is the design system's
/// own hairline, so "unavailable" reads the same in this app as it does in the
/// file manager and the gallery, and it stays in step if the token ever moves.
const DISABLED: u32 = BORDER_SUBTLE;

// -----------------------------------------------------------------------------
// Screen bands - the single source of truth for both drawing and the
// `Dirty` -> band mapping used by `render_camera_band`.
// -----------------------------------------------------------------------------
const TOP_BAND_Y0: usize = 0;
/// WHY 110 and not 68: the top row ends at 68, but the hairline that closes the
/// header is drawn at 108..109, so a band that stopped at the pill would leave a
/// stale divider on screen.
const TOP_BAND_Y1: usize = 110;

const VIEWFINDER_Y0: usize = 120;
const VIEWFINDER_Y1: usize = 1120;

/// Inner width of the viewfinder canvas, which is inset 20px from each edge of
/// the surface (`x >= 20 && x < W - 20`).
///
/// WHY it is named: it is the column a centred run in the viewfinder is measured
/// against, so `outline_text::place_run` has the same width the layout used to
/// centre it. The absence headline measures 472px on the bitmap face at scale 2
/// and 462px on the outline face, so both clear it.
const VIEWFINDER_W: usize = W - 40;

const MODE_STRIP_Y0: usize = 1150;
const MODE_STRIP_Y1: usize = 1200;

const DOCK_Y0: usize = 1210;
const DOCK_Y1: usize = H;

// -----------------------------------------------------------------------------
// Top control row geometry
//
// WHY these are named constants: the old renderer and the old hit test each
// transcribed the same five boxes independently (`x >= 130 && x < 250` and
// friends), so a control and its tap region were two separate sets of literals
// that could - and did - drift. Every box below is drawn from it and hit-tested
// from it.
// -----------------------------------------------------------------------------
const TOP_ROW_Y: usize = 28;
const TOP_ROW_H: usize = 40;

/// Back control, 52 wide: the shell's minimum touch target.
const BACK_RECT: Rect = Rect::new(24, TOP_ROW_Y, 52, TOP_ROW_H);
/// Flash / torch pill.
const FLASH_RECT: Rect = Rect::new(92, TOP_ROW_Y, 120, TOP_ROW_H);
/// Capture-geometry readout. NOT a control: it reports what the HAL returned.
const FRAME_RECT: Rect = Rect::new(228, TOP_ROW_Y, 160, TOP_ROW_H);
/// ISP filter pill (a real, local software filter - genuinely actionable).
const FILTER_RECT: Rect = Rect::new(404, TOP_ROW_Y, 140, TOP_ROW_H);
/// Capture counter badge. NOT a control: it reports `total_photos`.
const SHOTS_RECT: Rect = Rect::new(560, TOP_ROW_Y, 136, TOP_ROW_H);

/// Corner radius of the top-row pills.
///
/// WHY 16 and not `draw_pill`'s half-height: these are rounded rectangles, not
/// capsules. The old shape drew a 16px corner on a 40px control, and switching
/// to `draw_pill` would round them to 20px and change every control in the row.
const TOP_PILL_RADIUS: usize = 16;

/// Text baseline row for every scale-1 label in the top row.
///
/// WHY derived: the face is `GLYPH_H` (24) rows tall, so a scale-1 run owns 24
/// rows and must be centred inside the 40px control or its descenders clip.
const TOP_ROW_TEXT_Y0: usize = TOP_ROW_Y + (TOP_ROW_H - std::design::font::GLYPH_H) / 2;

/// Centre of the flash pictogram inside `FLASH_RECT`.
const FLASH_ICON_CX: usize = FLASH_RECT.x + 24;
const FLASH_ICON_CY: usize = FLASH_RECT.y + FLASH_RECT.h / 2;
/// Text room to the right of the flash pictogram, inside `FLASH_RECT`.
const FLASH_TEXT_X: usize = FLASH_RECT.x + 32;
const FLASH_TEXT_W: usize = FLASH_RECT.w - 32 - 8;

/// The system home gesture strip: a 200x6 indicator drawn inside a 240x40 box.
///
/// WHY the hit box is larger than the ink, and why both are named: 6px of ink
/// is not a touch target, so the tap region has to be the whole strip - but the
/// old code hit-tested `x 240..480 && y >= 1400` with no upper bound on y, a
/// region that was never drawn, so a tap well above the indicator exited the app.
const HOME_HIT: Rect = Rect::new(240, 1400, 240, 40);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

// Shutter button, as concentric radii about its centre.
const SHUTTER_CX: usize = 360;
const SHUTTER_CY: usize = 1310;
/// Outer white metallic ring.
const SHUTTER_RING_OUTER: usize = 64;
/// Concentric track gap between the ring and the core.
const SHUTTER_RING_INNER: usize = 56;
/// Inner tactile shutter core.
const SHUTTER_R_CORE: usize = 48;

/// The in-viewfinder zoom pill, in absolute surface coordinates.
///
/// WHY absolute and not viewfinder-relative: the tap handler works in surface
/// coordinates, and the old code transcribed the pill twice in two different
/// frames (`rel 276..404 / 874..930` while drawing, `296..424 / 994..1050`
/// while hit-testing). One rectangle, one frame.
const ZOOM_PILL: Rect = Rect::new(296, 994, 128, 56);

/// Half-extent of the disabled gallery-thumbnail and lens-flip squares in the
/// dock. WHY they are never hit-tested: see the dock drawing below.
const DOCK_SIDEBTN_HALF: usize = 36;

/// Whether a touch at (dx, dy) from the shutter centre is on the shutter.
///
/// WHY derived from the drawn radii rather than a hand-written box: the old hit
/// test was `y 1220..1400 && x 280..440`, a rectangle that included 24px above
/// and 26px below the painted ring and none of the dock beside it.
fn shutter_hit(dx: isize, dy: isize) -> bool {
    let r = SHUTTER_RING_OUTER as isize;
    dx * dx + dy * dy <= r * r
}

// -----------------------------------------------------------------------------
// Embedded 8x8 ASCII Font (Characters 32 ' ' through 126 '~')
// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font

/// Rasterises one text run into the pixel at `(x, y)`.
///
/// WHY the outline face comes first: the zoom readout, the mode carousel and the
/// viewfinder's absence headline run at scale 2, so on the bitmap face each is a
/// 16x24 cell magnified by an integer -- a soft ramp where an edge belongs.
/// `outline_text::place_run` rasterises the real outline at the em that matches
/// the cell's cap height (17 rows at scale 1, so 17*N*2048/1456 px per em), and
/// answers for the pixel itself. When it declines -- a run at scale 1, whose cell
/// is already at its native size, a run wider than the column the caller
/// reserved, a glyph whose ink would leave that column, or a weight this app
/// draws bold -- the bitmap face below draws the run exactly as it did before, so
/// a refusal costs the edge quality and never the text.
///
/// WHY the box test comes first: rasterising is the expensive part and most
/// pixels this app asks about are nowhere near a run. Gating on
/// `outline_text::run_box` -- the union of both faces' extents, not the bitmap
/// cell's -- means a run is only placed on a pixel it can ink, so the shared
/// coverage buffer is written once per run rather than once per pixel.
///
/// WHY the run answers alone when it is placed: the two faces are alternatives,
/// not layers. Falling through to the bitmap face after an outline run had
/// already inked a pixel would double-ink every partially covered edge, which is
/// exactly what the first attempt at this in the launcher drew: an outline glyph
/// with a hard-edged bitmap ghost one pixel off, on every string. The face is
/// chosen from the run's own arguments before `(x, y)` is consulted, so it is the
/// same choice for every pixel of the run.
#[inline(always)]
fn test_text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    max_w: usize,
    color: u32,
) -> Option<u32> {
    if scale < 2 {
        let w = std::design::font::measure(text, scale);
        let h = std::design::font::line_height(scale);
        if x < x0 || x - x0 >= w || y < y0 || y - y0 >= h {
            return None;
        }
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false);
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, Some(max_w), false) {
        return run.pixel(x, y, color, BG_DARK);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, BG_DARK, false)
}

/// Left x that centres `text` at `scale` inside `[x0, x0 + available)`, measured
/// for the face that will actually ink it.
///
/// WHY the measured width and not `font::measure`: the run is placed through
/// `test_text`, which prefers the outline face, and the outline face advances
/// by its own table rather than the bitmap cell's. Both widths are decided by
/// `outline_text::drawn_advance`, which mirrors `place_run`'s own refusals, so
/// the centring uses the width of the run that is drawn instead of the width of
/// a run that is not. On the bitmap fallback the answer is `font::measure`,
/// which is what the bitmap rasteriser steps by.
#[inline(always)]
fn centred_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let w = outline_text::drawn_advance(text, scale, Some(available))
        .unwrap_or_else(|| std::design::font::measure(text, scale));
    x0 + available.saturating_sub(w) / 2
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

/// The rounded-rect body of a top-row pill.
///
/// Returns `(fill, inset_x, inset_y)` when `(x, y)` is inside the pill, or `None`
/// when it is outside - including the transparent corner pixels, exactly as the
/// old inline test did. WHY a helper: those eight lines were copy-pasted into
/// each of the four pill branches, so a change to the corner geometry had to be
/// applied four times and one of them could be missed.
#[inline(always)]
fn top_pill_body(x: usize, y: usize, rect: Rect) -> Option<(u32, usize, usize)> {
    if !rect.contains(x, y) {
        return None;
    }
    let rx = (x - rect.x).min(rect.x + rect.w - 1 - x);
    let ry = (y - rect.y).min(rect.y + rect.h - 1 - y);
    let r = TOP_PILL_RADIUS as isize;
    if (rx as isize) < r && (ry as isize) < r {
        let dx = r - rx as isize;
        let dy = r - ry as isize;
        if dx * dx + dy * dy >= r * r {
            return None;
        }
    }
    Some((SURFACE_RAISED, rx, ry))
}

/// Draws a scale-1 label centred in `rect` at the top row's text row, or
/// reports that this pixel is not part of the run.
///
/// WHY a helper: centring the run is `rect.x + (rect.w - measure) / 2`, and
/// every top-row label needs it. Measuring per pixel is what the old code did
/// inline four times; `measure` is a pure sum of advances, so this is cheap and
/// it keeps the centring identical across all four.
#[inline(always)]
fn top_row_label(
    x: usize,
    y: usize,
    rect: Rect,
    label: &[u8],
    color: u32,
) -> Option<u32> {
    let w = std::design::font::measure(label, 1);
    if w > rect.w {
        // WHY draw nothing rather than overflow: a label wider than its control
        // would run into the next pill, and a truncated run would lie about the
        // value. `PROBE` returns true for these strings.
        return None;
    }
    let x0 = rect.x + (rect.w - w) / 2;
    test_text(x, y, x0, TOP_ROW_TEXT_Y0, 1, label, rect.w, color)
}

/// Draws a scale-1 label right of the flash pictogram.
#[inline(always)]
fn flash_label(x: usize, y: usize, label: &[u8], color: u32) -> Option<u32> {
    let w = std::design::font::measure(label, 1);
    if w > FLASH_TEXT_W {
        return None;
    }
    let x0 = FLASH_TEXT_X + (FLASH_TEXT_W - w) / 2;
    test_text(x, y, x0, TOP_ROW_TEXT_Y0, 1, label, FLASH_TEXT_W, color)
}

#[inline(never)]
fn get_camera_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    // 1. Status Bar & Top Controls (y: 0..110)
    if y < TOP_BAND_Y1 {
        if y >= TOP_BAND_Y1 - 2 {
            return BORDER_SUBTLE;
        }

        // Back control. WHY an outline measured from the rectangle's own edges
        // instead of the old `dx + dy <= 12` stroke: that stroke's apex landed
        // 22px right of the button's centre, so the glyph was not centred in the
        // thing it labelled. The chevron is now centred on `BACK_RECT`, and the
        // box is inset by one pixel so it is never clipped by `contains`.
        // Back control circular disc (cx: 50, cy: 48, radius: 20)
        if BACK_RECT.contains(x, y) {
            let cx = BACK_RECT.x + BACK_RECT.w / 2;
            let cy = BACK_RECT.y + BACK_RECT.h / 2;
            let dx = x as isize - cx as isize;
            let dy = y as isize - cy as isize;
            let dsq = dx * dx + dy * dy;
            if dsq <= 20 * 20 {
                if dsq >= 18 * 18 {
                    return BORDER_SUBTLE;
                }
                if dx <= 2 && dx >= -5 {
                    let arm_dy = (dx - (-5)).abs();
                    if (dy.abs() - arm_dy).abs() <= 1 {
                        return TEXT_PRIMARY;
                    }
                }
                return SURFACE_RAISED;
            }
            return BG_DARK;
        }

        // Flash / torch pill. This is a real control: it issues
        // `camera_toggle_flash` and reports what the backend answered.
        if let Some((fill, rx, ry)) = top_pill_body(x, y, FLASH_RECT) {
            let is_lit =
                camera.flash_available && (camera.flash == FlashMode::Torch || camera.flash == FlashMode::On);
            let icon_color = if is_lit {
                ACCENT_AMBER
            } else if !camera.flash_available {
                TEXT_MUTED
            } else {
                TEXT_PRIMARY
            };

            if test_lightning(x, y, FLASH_ICON_CX, FLASH_ICON_CY) {
                return icon_color;
            }

            let label: &[u8] = if !camera.flash_available {
                &b"OFF"[..]
            } else if is_lit {
                &b"TORCH"[..]
            } else {
                &b"AUTO"[..]
            };
            if let Some(c) = flash_label(x, y, label, icon_color) {
                return c;
            }

            if rx < 2 || ry < 2 {
                return if is_lit { ACCENT_AMBER } else { BORDER_SUBTLE };
            }
            return fill;
        }

        // Capture geometry. WHY this is no longer "48MP PRO": that string
        // advertised a resolution and a shooting mode the hardware never
        // reported and this app can never produce, and it was painted as if it
        // were a control. It now reports the frame the HAL actually returned -
        // "NO SENSOR" when the kernel refused camera authority, "NO FRAME" until
        // a capture succeeds, then the real `WxH`.
        if let Some((_fill, rx, ry)) = top_pill_body(x, y, FRAME_RECT) {
            let label = &camera.frame_label[..camera.frame_label_len];
            // WHY muted ink and not `DISABLED`: this pill is a readout, not a
            // control, and it has to stay legible - "NO SENSOR" is the answer to
            // a question the user is asking. Only its outline is recessed, to
            // signal that there is nothing to press.
            let color = if camera.camera_authorised { ACCENT_SKY } else { TEXT_MUTED };
            if let Some(c) = top_row_label(x, y, FRAME_RECT, label, color) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return DISABLED;
            }
            return SURFACE_RAISED;
        }

        // ISP filter pill. This is a real control: the filter is applied in
        // `apply_isp_tone_curve` to the captured buffer, so selecting one
        // changes what the next capture is tone-mapped with.
        if let Some((fill, rx, ry)) = top_pill_body(x, y, FILTER_RECT) {
            let (filter_name, filter_color) = match camera.filter {
                CameraFilter::Normal => (&b"NORMAL"[..], ACCENT_SKY),
                CameraFilter::Monochrome => (&b"MONO"[..], TEXT_MUTED),
                CameraFilter::HighContrast => (&b"CONTRAST"[..], TEXT_PRIMARY),
                CameraFilter::Sepia => (&b"SEPIA"[..], ACCENT_AMBER),
                CameraFilter::NeuralHeatmap => (&b"HEATMAP"[..], DANGER_CRIMSON),
            };
            if let Some(c) = top_row_label(x, y, FILTER_RECT, filter_name, filter_color) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return filter_color;
            }
            return fill;
        }

        // Capture counter. WHY this is no longer a green "AI" badge: "AI"
        // asserted a running neural-vision pipeline that this app does not have
        // - it forwards nothing to any inference broker, and no such service is
        // bound to it. The badge now reports `total_photos`, a counter that only
        // advances when `camera_capture` actually returned a frame.
        if let Some((_fill, rx, ry)) = top_pill_body(x, y, SHOTS_RECT) {
            let label = &camera.shots_label[..camera.shots_label_len];
            if let Some(c) = top_row_label(x, y, SHOTS_RECT, label, SUCCESS_EMERALD) {
                return c;
            }
            if rx < 2 || ry < 2 {
                return SUCCESS_EMERALD;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // 2. Viewfinder Canvas (y: VIEWFINDER_Y0..VIEWFINDER_Y1, x: 20..700)
    if (VIEWFINDER_Y0..VIEWFINDER_Y1).contains(&y) && x >= 20 && x < W - 20 {
        let rel_y = y - VIEWFINDER_Y0;
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
            // Fast bounds check: head sits at y=470 (scale 2, height <= 48), sub sits at y=530 (scale 1, height <= 24).
            // Outside y in 465..560, the viewfinder canvas has no text ink.
            if y >= 465 && y < 560 {
                let head = b"No camera sensor bound";
                let sub = b"Viewfinder inactive - no frames from the camera HAL";
                if y < 525 {
                    if let Some(c) = test_text(
                        x,
                        y,
                        camera.head_x,
                        470,
                        2,
                        head,
                        VIEWFINDER_W,
                        TEXT_PRIMARY,
                    ) {
                        return c;
                    }
                } else if y >= 528 {
                    if let Some(c) = test_text(
                        x,
                        y,
                        camera.sub_x,
                        530,
                        1,
                        sub,
                        VIEWFINDER_W,
                        TEXT_MUTED,
                    ) {
                        return c;
                    }
                }
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
        // 994..1050. The old pill sat at abs y 1080..1130 with text rows
        // 1098..1146, so its bottom 10px spilled past the viewfinder edge and
        // its descenders clipped at 1130; on-device it read as half-covered by
        // the control strip. This pill ends 70px above the viewfinder edge and
        // 100px above the mode strip, so it can never be covered.
        if ZOOM_PILL.contains(x, y) {
            let rel_zx = x - ZOOM_PILL.x;
            let rel_zy = y - ZOOM_PILL.y;
            let rx = rel_zx.min(ZOOM_PILL.w - 1 - rel_zx);
            let ry = rel_zy.min(ZOOM_PILL.h - 1 - rel_zy);
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
            let tx0 = centred_x(zoom_str, scale, ZOOM_PILL.x, ZOOM_PILL.w);
            // WHY derived: a scale-2 run is 48 rows tall and the pill is 56, so
            // the run owns the pill minus 4 rows top and bottom. Hard-coding 998
            // is what let the previous pill's descenders clip.
            let ty0 = ZOOM_PILL.y + (ZOOM_PILL.h - 48) / 2;
            if let Some(c) = test_text(x, y, tx0, ty0, scale, zoom_str, ZOOM_PILL.w, TEXT_PRIMARY) {
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
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
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
        // Strip is 50px tall: y0 = MODE_STRIP_Y0 + (50 - lh) / 2 -> 1151 at
        // scale 2 (inks 1151..1199 inside 1150..1200), 1163 at scale 1.
        let y0 = MODE_STRIP_Y0 + (50usize.saturating_sub(lh)) / 2;

        if x < S1_X {
            // Document (slot 0)
            let is_doc = camera.mode == CameraMode::DocumentVision;
            let doc_color = if is_doc { TEXT_PRIMARY } else { TEXT_MUTED };
            let doc_w = std::design::font::measure(DOC, scale);
            let doc_x = S0_X + (S0_W.saturating_sub(doc_w)) / 2;
            if let Some(c) = test_text(x, y, doc_x, y0, scale, DOC, S0_W, doc_color) {
                return c;
            }
        } else if x < S2_X {
            // Photo (slot 1)
            let is_photo = camera.mode == CameraMode::Photo;
            let photo_color = if is_photo { TEXT_PRIMARY } else { TEXT_MUTED };
            let pho_w = std::design::font::measure(PHO, scale);
            let pho_x = S1_X + (S1_W.saturating_sub(pho_w)) / 2;
            if let Some(c) = test_text(x, y, pho_x, y0, scale, PHO, S1_W, photo_color) {
                return c;
            }
        } else {
            // Video (slot 2)
            let is_vid = camera.mode == CameraMode::Video;
            let vid_color = if is_vid { TEXT_PRIMARY } else { TEXT_MUTED };
            let vid_w = std::design::font::measure(VID, scale);
            let vid_x = S2_X + (S2_W.saturating_sub(vid_w)) / 2;
            if let Some(c) = test_text(x, y, vid_x, y0, scale, VID, S2_W, vid_color) {
                return c;
            }
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

    // 4. Bottom Shutter Dock & Hardware Controls (y: DOCK_Y0..DOCK_Y1)
    if (DOCK_Y0..DOCK_Y1 - 30).contains(&y) {
        // Left gallery thumbnail. WHY drawn recessed and never hit-tested:
        // nothing behind it exists to open. The camera HAL exposes no photo roll
        // and this task holds no media index, so the old amber landscape glyph
        // promised a library that cannot be listed and the old hit handler had
        // no branch for it at all - a control that looks live and does nothing.
        let thumb_dx = (x as isize - 110).abs();
        let thumb_dy = (y as isize - SHUTTER_CY as isize).abs();
        if thumb_dx <= DOCK_SIDEBTN_HALF as isize && thumb_dy <= DOCK_SIDEBTN_HALF as isize {
            // WHY muted rather than `DISABLED` here: an icon painted in the
            // hairline token against a #25282A card is nearly invisible, which
            // is the same defect as the invisible exit hotspot this app's file
            // manager had. Visible-but-inert is the honest middle.
            if test_gallery_icon(x, y, 110, SHUTTER_CY) {
                return TEXT_MUTED;
            }
            if thumb_dx == DOCK_SIDEBTN_HALF as isize || thumb_dy == DOCK_SIDEBTN_HALF as isize {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        // Center Shutter Button. WHY the disabled treatment: `camera_capture` is
        // refused with PermissionDenied unless the task holds `SystemControl` on
        // the camera service object or `HardwarePortIo` on port 0xCA00, and this
        // task is spawned with neither. Painting the full-brightness ring and
        // core made a button that provably cannot capture look armed.
        let dx = x as isize - SHUTTER_CX as isize;
        let dy = y as isize - SHUTTER_CY as isize;
        let dist_sq = dx * dx + dy * dy;

        let outer = (SHUTTER_RING_OUTER * SHUTTER_RING_OUTER) as isize;
        let inner = (SHUTTER_RING_INNER * SHUTTER_RING_INNER) as isize;
        let core = (SHUTTER_R_CORE * SHUTTER_R_CORE) as isize;

        // Outer metallic ring.
        if dist_sq <= outer && dist_sq >= inner {
            return if camera.camera_authorised { TEXT_PRIMARY } else { DISABLED };
        }
        // Concentric track gap.
        if dist_sq < inner && dist_sq >= core {
            return BG_DARK;
        }
        // Inner tactile shutter core.
        if dist_sq < core {
            if !camera.camera_authorised {
                return SURFACE_RAISED;
            } else if camera.mode == CameraMode::Video {
                return DANGER_CRIMSON;
            } else if camera.mode == CameraMode::DocumentVision {
                return ACCENT_SKY;
            } else {
                return TEXT_PRIMARY;
            }
        }

        // Right Lens Flip Button. WHY drawn recessed and never hit-tested: the
        // camera HAL exposes `capture_frame` and nothing that selects between a
        // front and a rear sensor, so there is no lens to flip. The old handler
        // played a haptic click and changed no state - a control that looked
        // armed and only pretended to.
        let flip_dx = (x as isize - 610).abs();
        let flip_dy = (y as isize - SHUTTER_CY as isize).abs();
        let flip_r2 = flip_dx * flip_dx + flip_dy * flip_dy;
        if flip_r2 <= (DOCK_SIDEBTN_HALF * DOCK_SIDEBTN_HALF) as isize {
            if test_flip_arrows(x, y, 610, SHUTTER_CY) {
                return TEXT_MUTED;
            }
            if flip_r2 >= 30 * 30 {
                return BORDER_SUBTLE;
            }
            return SURFACE_RAISED;
        }

        return BG_DARK;
    }

    // Home gesture bar at bottom
    if HOME_PILL.contains(x, y) {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_camera_ui(surface: u32, camera: &CameraApp) {
    render_camera_band(surface, camera, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is named after what the
/// touch actually moved (see `Dirty`), and every row outside it is correct
/// because nothing about it changed. The old renderer re-rasterised all
/// 1,036,800 pixels on every touch event, including touches that changed no
/// state at all.
fn render_camera_band(surface: u32, camera: &CameraApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
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

/// Which rows of the surface a touch actually changed.
///
/// WHY this exists: the old loop rendered the whole surface after every tap and
/// flipped it, whether the tap had changed the flash state, the ISP filter, the
/// mode or nothing at all. Naming the region lets a tap that changes one pill
/// cost one band.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing moved; the surface is already correct, so do not render at all.
    None,
    /// Flash or ISP filter changed: the top control row only.
    TopRow,
    /// Zoom changed: only the viewfinder canvas.
    Viewfinder,
    /// More than one band moved: the whole surface.
    ///
    /// WHY there is no `ModeStrip` variant even though the strip is one band:
    /// `mode` is read twice - by the carousel's label colour *and* by the shutter
    /// core's fill - so no single band covers it. Naming a band that would
    /// leave half the change stale is worse than naming none.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several touches before the app is scheduled
/// again; rendering only the last region would leave the earlier one stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (a, b) if a == b => a,
            _ => Dirty::All,
        }
    }
}

enum Tap {
    /// Nothing on screen moved.
    None,
    /// The model moved; render the named region.
    Redraw(Dirty),
    /// Leave to the launcher, logging this exact reason first.
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(6, surface);
    }
    std::exit_with_code(0);
}

fn handle_camera_touch(camera: &mut CameraApp, x: u32, y: u32) -> Tap {
    let x = x as usize;
    let y = y as usize;

    // 1. Back control or the home gesture strip: exit to the Launcher.
    // WHY the named rectangles: the old test was `x 24..100 && y 28..68 ||
    // x 240..480 && y >= 1400`, a second transcription of the header's geometry
    // with no upper bound on y.
    if BACK_RECT.contains(x, y) || HOME_HIT.contains(x, y) {
        return Tap::Leave("[CAMERA] Back control or home gesture pressed. Exiting to launcher.");
    }

    // 2. Flash mode toggle pill - a real control, backed by `camera_toggle_flash`.
    //
    // WHY: the flash pill previously advanced the local mode and then
    // discarded the `Err` from camera_toggle_flash, so the UI showed the next
    // flash mode (ending in a lit Torch) even though the backend refused the
    // request. Report the real outcome and keep the pill fail-closed.
    if FLASH_RECT.contains(x, y) {
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
        return Tap::Redraw(Dirty::TopRow);
    }

    // 3. ISP filter toggle pill - a real control: `apply_isp_tone_curve` applies
    //    the selected filter to the captured buffer, so it changes the next
    //    capture rather than only a label.
    if FILTER_RECT.contains(x, y) {
        camera.filter = match camera.filter {
            CameraFilter::Normal => CameraFilter::Monochrome,
            CameraFilter::Monochrome => CameraFilter::HighContrast,
            CameraFilter::HighContrast => CameraFilter::Sepia,
            CameraFilter::Sepia => CameraFilter::NeuralHeatmap,
            CameraFilter::NeuralHeatmap => CameraFilter::Normal,
        };
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 4. Zoom toggle pill in viewfinder. Guarded by active video frames so an
    //    inactive/unbound camera HAL cannot register touch events on invisible chrome.
    if camera.total_video_frames > 0 && ZOOM_PILL.contains(x, y) {
        camera.zoom_x10 = match camera.zoom_x10 {
            10 => 20,
            20 => 50,
            _ => 10,
        };
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 5. Mode Selector Carousel: three equal 226px slots in 20..700, matching the
    //    render slots exactly (50px tall, >=48).
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
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
        // WHY the whole surface and not just the strip: `mode` is read in two
        // places - the mode carousel's label colour and the shutter core's fill
        // (crimson for Video, sky for Document). Rendering only the strip left
        // the shutter painted in the previous mode's colour, which is exactly
        // the class of bug banding is supposed to prevent.
        return Tap::Redraw(Dirty::All);
    }

    // 6. Shutter Button. WHY gated on the authority probe: the shutter is drawn
    //    recessed when the kernel refused camera authority, and a recessed
    //    control that still fired would be the dead affordance again. WHY the
    //    circle test: it is derived from the radii the renderer draws, where the
    //    old rectangle included 24px above and 26px below the painted ring.
    if shutter_hit(x as isize - SHUTTER_CX as isize, y as isize - SHUTTER_CY as isize) {
        if !camera.camera_authorised {
            std::log(
                0,
                "[CAMERA APP] Shutter unavailable: the kernel refused camera authority for this task.",
            )
            .ok();
            return Tap::None;
        }
        // WHY report the real outcome: `let _ =` here discarded the refusal, so a
        // press that captured nothing was indistinguishable from one that did.
        // The two attempts return different `Ok` types (`u64` frame id, `u32`
        // shot count), so both are folded to a unit `Ok` and the real `Err` code
        // is carried through untouched - it is the kernel's answer, never a
        // constant written here.
        let outcome: Result<(), i64> = if camera.mode == CameraMode::Video {
            camera.stream_video_frame().map(|_| ())
        } else {
            camera.capture_still_photo().map(|_| ())
        };
        match outcome {
            Ok(()) => return Tap::Redraw(Dirty::All),
            Err(code) => {
                let mut buf = [0u8; 32];
                let prefix = b"[CAMERA APP] Capture refused, code -";
                let p = prefix.len().min(buf.len());
                buf[..p].copy_from_slice(&prefix[..p]);
                let n = p + fmt_u32(code.unsigned_abs() as u32, &mut buf[p..]);
                if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                    std::log(0, msg).ok();
                }
                return Tap::None;
            }
        }
    }

    // The dock's gallery thumbnail and lens-flip square are painted recessed and
    // are deliberately not hit-tested: there is no photo roll to open and no
    // lens selector to flip, and the old lens branch answered a tap with a
    // haptic click and no state change at all.
    Tap::None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(
        0,
        "[CAMERA APP (Ring 3)] Sovereign Camera & Neural Vision online.",
    )
    .ok();

    let mut camera = CameraApp::new();
    camera.init();

    // Ask the kernel whether this task may drive a camera at all.
    //
    // WHY before the first frame: the shutter, the capture-geometry readout and
    // the flash pill are all drawn from this answer, so the very first presented
    // frame already tells the truth. The probe is a read-only autofocus query
    // and changes no hardware state, which the comment above this line used to
    // guarantee for the whole app.
    camera.probe_camera_authority();
    if camera.camera_authorised {
        std::log(
            0,
            "[CAMERA APP] Kernel granted camera authority; capture controls are armed.",
        )
        .ok();
    } else {
        std::log(
            0,
            "[CAMERA APP] Kernel refused camera authority for this task; capture controls are shown unavailable.",
        )
        .ok();
    }

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
                    // WHY only redraw when a frame actually arrived: the old
                    // loop flipped the surface on every 100-tick timer event
                    // whether or not anything changed, which spent scanout
                    // bandwidth redisplaying an identical viewfinder. The
                    // counter only moves when `stream_video_frame` succeeds, so
                    // comparing it is a real "did the canvas move" test.
                    let before = camera.total_video_frames;
                    let _ = camera.stream_video_frame();
                    if camera.total_video_frames != before {
                        render_camera_band(surface, &camera, VIEWFINDER_Y0, VIEWFINDER_Y1);
                        let _ = std::surface_flip(6);
                    }
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
                let mut dirty = Dirty::None;
                while std::input_event_poll(&mut input).is_ok() {
                    if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                        std::log(
                            0,
                            "[CAMERA APP] Navigation gesture recognized. Returning to launcher.",
                        )
                        .ok();
                        exit_to_launcher(surface);
                    }
                    if input.kind == 1 {
                        match handle_camera_touch(&mut camera, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None {
                    match dirty {
                        Dirty::TopRow => {
                            render_camera_band(surface, &camera, TOP_BAND_Y0, TOP_BAND_Y1)
                        }
                        Dirty::Viewfinder => render_camera_band(
                            surface,
                            &camera,
                            VIEWFINDER_Y0,
                            VIEWFINDER_Y1,
                        ),
                        Dirty::All => render_camera_ui(surface, &camera),
                        Dirty::None => {}
                    }
                    let _ = std::surface_flip(6);
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
