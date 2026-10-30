//! InterOS camera application, executing in Ring 3 (User Mode).
//!
//! - A capture engine that takes a still photograph and writes it into the
//!   shared storage area as a PPM (P6), and publishes it into the platform's
//!   document index, so the gallery can open the photo the user just took
//! - A mode carousel of Photo, Video and Document framing with Scale 2 Body
//!   typography and touch targets >= 48px
//! - An image signal processor (ISP) providing 6 calibrated filter presets:
//!   Normal, Vivid, Warm, Cool, Mono, and Vintage
//! - Rule-of-Thirds 3x3 framing grid and Aspect Ratio selection (4:3, 16:9, 1:1)
//! - Flash modes (Auto, On, Off) with responsive visual pill indicators
//! - Bottom-left Gallery thumbnail disc showing live preview of the last
//!   captured photo and launching the Gallery app via intent or package launch
//! - DRM/KMS client surface rendering (z=200) with banded re-rasterisation
//! - Zero heap allocations in interactive rendering and event reactor loops

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use camera::theme::{chrome, radius, spacing, typography, Theme, THEME};

const STATUS_BAR_HEIGHT: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

fn refresh_clock(camera: &mut CameraApp) {
    if let Some(now) = std::rtc::Reading::now() {
        camera.clock = now.clock_text();
        camera.clock_ok = true;
    } else {
        camera.clock_ok = false;
    }
}
use std::design::primitives::Rect;

use interos_std::design::outline_text;

use camera::{
    CameraApp, CameraAspectRatio, CameraFilter, CameraMode, CameraScreen, CameraStatus, CaptureOutcome, FlashMode,
    SettingsRow, TimerMode, SETTINGS_ROW_ORDER, THUMBNAIL_DIM,
};

/// The platform's corner radii, under the short names the renderer uses.
const R_CARD: usize = radius::CARD;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// Screen bands
// -----------------------------------------------------------------------------
const TOP_BAND_Y0: usize = 0;
const TOP_BAND_Y1: usize = 112;

/// The viewfinder and the dock take their geometry from the model, so the
/// rectangles the renderer paints are the rectangles the touch handler taps.
const VIEWFINDER_Y0: usize = camera::VIEWFINDER_TOP;
const VIEWFINDER_Y1: usize = camera::VIEWFINDER_BOTTOM;

/// Every control aligns to the platform's 24px gutter.
const GUTTER: usize = spacing::GRID_24;
const VIEWFINDER_X0: usize = camera::VIEWFINDER_LEFT;
const VIEWFINDER_X1: usize = camera::VIEWFINDER_RIGHT;

/// Inner width of the viewfinder canvas (24px gutter on each edge).
const VIEWFINDER_W: usize = camera::VIEWFINDER_WIDTH;

/// The three chips over the viewfinder, in their row order.
/// Aspect ratio toggle (4:3 vs 16:9 vs 1:1).
const ASPECT_RECT: Rect = camera::chip_rect(0);
/// Rule-of-Thirds 3x3 framing grid toggle.
const GRID_RECT: Rect = camera::chip_rect(1);
/// Self-timer mode toggle.
const TIMER_RECT: Rect = camera::chip_rect(2);

const MODE_STRIP_Y0: usize = 1144;
const MODE_STRIP_Y1: usize = 1204;

/// Three equal mode slots, 12px apart, inside the 24px gutters.
const MODE_SLOT_W: usize = 216;
const MODE_SLOT_GAP: usize = 12;
const MODE_X0: usize = GUTTER;
const MODE_X1: usize = MODE_X0 + MODE_SLOT_W + MODE_SLOT_GAP;
const MODE_X2: usize = MODE_X1 + MODE_SLOT_W + MODE_SLOT_GAP;

const DOCK_Y0: usize = camera::DOCK_Y0;
const DOCK_Y1: usize = H;

// -----------------------------------------------------------------------------
// Top control row geometry (height 48px, all >= 48px touch affordance)
// -----------------------------------------------------------------------------
/// WHY the row starts at HEADER_TOP and not at 24: the band owns the first 48
/// rows and answers the pixel function before this row is reached, so half of
/// every control here was painted over.
const TOP_ROW_Y: usize = interos_std::design::viewport::HEADER_TOP;
const TOP_ROW_H: usize = 48;

/// Back control, 48x48: matches the SDK back button's painted box exactly.
const BACK_RECT: Rect = Rect::new(GUTTER, TOP_ROW_Y, 48, TOP_ROW_H);
/// Flash / torch pill: 116x48 (>=48px).
const FLASH_RECT: Rect = Rect::new(80, TOP_ROW_Y, 116, TOP_ROW_H);
/// Capture-geometry readout: 150x48. Reports platform geometry.
const FRAME_RECT: Rect = Rect::new(204, TOP_ROW_Y, 150, TOP_ROW_H);
/// ISP filter pill: 170x48 (>=48px).
const FILTER_RECT: Rect = Rect::new(362, TOP_ROW_Y, 170, TOP_ROW_H);
/// Capture counter badge: 156x48. Reports `total_photos`.
const SHOTS_RECT: Rect = Rect::new(540, TOP_ROW_Y, 156, TOP_ROW_H);

/// Corner radius of the floating pills.
const TOP_PILL_RADIUS: usize = radius::SOFT_PILL;

/// The status card that carries the platform's own verdict on the camera.
const STATUS_CARD_W: usize = 520;
const STATUS_CARD_H: usize = 124;
/// The card's text column: a 64px icon gutter and a 24px right margin.
const STATUS_TEXT_W: usize = STATUS_CARD_W - 88;

/// Centre of the flash pictogram inside `FLASH_RECT`.
const FLASH_ICON_CX: usize = FLASH_RECT.x + 24;
const FLASH_ICON_CY: usize = FLASH_RECT.y + FLASH_RECT.h / 2;
/// Text room to the right of the flash pictogram, inside `FLASH_RECT`.
const FLASH_TEXT_X: usize = FLASH_RECT.x + 36;
const FLASH_TEXT_W: usize = FLASH_RECT.w.saturating_sub(44);

/// The system home gesture strip: a 200x6 indicator drawn inside a 240x48 box.
const HOME_HIT: Rect = Rect::new(240, 1392, 240, 48);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

// Shutter button (r=48 core, outer ring 64px), as the model places it.
const SHUTTER_CX: usize = camera::SHUTTER_CX;
const SHUTTER_CY: usize = camera::SHUTTER_CY;
const SHUTTER_RING_OUTER: usize = camera::SHUTTER_RING_OUTER;
const SHUTTER_RING_INNER: usize = camera::SHUTTER_RING_INNER;
const SHUTTER_R_CORE: usize = camera::SHUTTER_R_CORE;

/// The dock's two side controls, as the centres of their pictograms.
const PHOTOS_CX: usize = 110;
const SETTINGS_CX: usize = 610;

/// Half-extent of the dock's side buttons (diameter 64px >= 48px).
const DOCK_SIDEBTN_HALF: usize = 32;

// -----------------------------------------------------------------------------
// The settings surface.
// -----------------------------------------------------------------------------
const SHEET_RECT: Rect = Rect::new(24, 140, 672, 960);
const SHEET_TITLE_Y0: usize = 160;
const SHEET_ROW_Y0: usize = 200;
const SHEET_ROW_H: usize = 96;
const SHEET_TEXT_X: usize = 48;
const SHEET_TEXT_W: usize = camera::SETTINGS_TEXT_MAX;
const SHEET_ROW_X0: usize = 36;
const SHEET_ROW_W: usize = 648;
const SHEET_ROW_GAP: usize = 8;

const RETICLE_ARM: usize = 40;

/// Whether a touch at (dx, dy) from the shutter centre is on the shutter.
fn shutter_hit(dx: isize, dy: isize) -> bool {
    let r = SHUTTER_RING_OUTER as isize;
    dx * dx + dy * dy <= r * r
}

/// Whether a touch at (x, y) is on one of the dock's side buttons.
fn side_button_hit(x: usize, y: usize, cx: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - SHUTTER_CY as isize;
    (dx * dx + dy * dy) <= (DOCK_SIDEBTN_HALF * DOCK_SIDEBTN_HALF) as isize
}

// -----------------------------------------------------------------------------
// Typography provided by interos_std::design::font & outline_text
// -----------------------------------------------------------------------------

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
    bg: u32,
) -> Option<u32> {
    if scale < 2 {
        let w = std::design::font::measure(text, scale);
        let h = std::design::font::line_height(scale);
        if x < x0 || x - x0 >= w.min(max_w) || y < y0 || y - y0 >= h {
            return None;
        }
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false);
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
        return None;
    }
    if x >= x0 + max_w {
        return None;
    }
    if let Some(run) = outline_text::place_run(x0, y0, scale, text, Some(max_w), false) {
        return run.pixel(x, y, color, bg);
    }
    std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false)
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

/// Vector landscape pictogram for the photo library button.
fn test_gallery_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 16 && dy.abs() <= 13 {
        if dx.abs() == 16 || dy.abs() == 13 {
            return true;
        }
        // Sun circle at (-7, -5), r=3
        let sun_dx = dx + 7;
        let sun_dy = dy + 5;
        if sun_dx * sun_dx + sun_dy * sun_dy <= 9 {
            return true;
        }
        // Mountain peaks
        if dy >= -1 && dy <= 10 {
            let m1 = (dx + 4).abs() * 2 <= (dy + 4);
            let m2 = (dx - 7).abs() * 2 <= (dy + 2);
            if m1 || m2 {
                return true;
            }
        }
    }
    false
}

/// Vector sliders pictogram for the settings button.
fn test_settings_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let rows = [(-10i32, -6i32), (0i32, 6i32), (10i32, -2i32)];
    for (row_y, knob_x) in rows {
        let ry = dy - row_y as isize;
        if ry.abs() <= 1 && dx.abs() <= 14 {
            return true;
        }
        let kx = dx - knob_x as isize;
        if kx * kx + ry * ry <= 12 {
            return true;
        }
    }
    false
}

/// The rounded-rect body of a top-row pill.
#[inline(always)]
fn top_pill_body(x: usize, y: usize, rect: Rect) -> Option<(u32, bool)> {
    if !rect.contains(x, y) {
        return None;
    }
    let rx = (x - rect.x).min(rect.x + rect.w - 1 - x);
    let ry = (y - rect.y).min(rect.y + rect.h - 1 - y);
    let r = TOP_PILL_RADIUS as isize;
    let mut on_border = false;
    if (rx as isize) < r && (ry as isize) < r {
        let dx = r - rx as isize;
        let dy = r - ry as isize;
        let d2 = dx * dx + dy * dy;
        if d2 >= r * r {
            return None;
        }
        if d2 >= (r - 2) * (r - 2) {
            on_border = true;
        }
    } else if rx < 2 || ry < 2 {
        on_border = true;
    }
    Some((chrome::SURFACE, on_border))
}

/// Draws a label centred in `rect`, upgrading to Scale 2 Body where it fits.
#[inline(always)]
fn top_row_label(
    x: usize,
    y: usize,
    rect: Rect,
    label: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let scale = if std::design::font::measure(label, 2) <= rect.w.saturating_sub(12) {
        2
    } else {
        1
    };
    let w = std::design::font::measure(label, scale);
    if w > rect.w {
        return None;
    }
    let x0 = rect.x + (rect.w - w) / 2;
    let lh = std::design::font::line_height(scale);
    let y0 = rect.y + (rect.h.saturating_sub(lh)) / 2;
    test_text(x, y, x0, y0, scale, label, rect.w, color, bg)
}

/// Draws a label right of the flash pictogram, upgrading to Scale 2 Body where it fits.
#[inline(always)]
fn flash_label(x: usize, y: usize, label: &[u8], color: u32, bg: u32) -> Option<u32> {
    let scale = if std::design::font::measure(label, 2) <= FLASH_TEXT_W {
        2
    } else {
        1
    };
    let w = std::design::font::measure(label, scale);
    if w > FLASH_TEXT_W {
        return None;
    }
    let x0 = FLASH_TEXT_X + (FLASH_TEXT_W - w) / 2;
    let lh = std::design::font::line_height(scale);
    let y0 = FLASH_RECT.y + (FLASH_RECT.h.saturating_sub(lh)) / 2;
    test_text(x, y, x0, y0, scale, label, FLASH_TEXT_W, color, bg)
}

/// The ink of the thirds grid: a quiet hairline on the well.
fn grid_ink() -> u32 {
    Theme::blend(chrome::WELL, chrome::TEXT_MUTED, 34)
}

/// The colour that states the platform's verdict, in the platform's roles.
fn status_ink(status: CameraStatus) -> u32 {
    match status {
        CameraStatus::AuthorityRefused | CameraStatus::Failed => chrome::REFUSED,
        CameraStatus::NoSensor => chrome::CAUTION,
        CameraStatus::AuthorityHeld => chrome::READY,
    }
}

/// The header overlay pills: aspect, grid and timer.
///
/// WHY this is painted after the letterbox: the pills float above the frame,
/// so a 4:3 or 1:1 matte must not erase half of them.
fn viewfinder_header_pixel(x: usize, y: usize, camera: &CameraApp) -> Option<u32> {
    if ASPECT_RECT.contains(x, y) {
        let fill = chrome::tint(chrome::SKY, 12);
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            ASPECT_RECT,
            TOP_PILL_RADIUS,
            fill,
            Some(chrome::tint(chrome::SKY, 70)),
            1,
        )?;
        let lbl = camera.aspect_ratio.label();
        return Some(top_row_label(x, y, ASPECT_RECT, lbl, chrome::SKY, fill).unwrap_or(c));
    }

    if GRID_RECT.contains(x, y) {
        let active = camera.guides;
        let fill = if active { chrome::tint(chrome::SKY, 14) } else { chrome::SURFACE };
        let ink = if active { chrome::SKY } else { chrome::TEXT_MUTED };
        let border = if active { chrome::SKY } else { chrome::BORDER };
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            GRID_RECT,
            TOP_PILL_RADIUS,
            fill,
            Some(border),
            1,
        )?;
        return Some(top_row_label(x, y, GRID_RECT, b"GRID", ink, fill).unwrap_or(c));
    }

    if TIMER_RECT.contains(x, y) {
        let active = camera.timer_mode != TimerMode::Off;
        let fill = if active { chrome::tint(chrome::SKY, 14) } else { chrome::SURFACE };
        let ink = if active { chrome::SKY } else { chrome::TEXT_MUTED };
        let border = if active { chrome::SKY } else { chrome::BORDER };
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            TIMER_RECT,
            TOP_PILL_RADIUS,
            fill,
            Some(border),
            1,
        )?;
        let lbl = camera.timer_mode.label();
        return Some(top_row_label(x, y, TIMER_RECT, lbl, ink, fill).unwrap_or(c));
    }
    None
}

/// The mode carousel: the selected mode sits in an amber capsule.
fn mode_strip_pixel(x: usize, y: usize, camera: &CameraApp) -> Option<u32> {
    const DOC: &[u8] = b"DOCUMENT";
    const PHO: &[u8] = b"PHOTO";
    const VID: &[u8] = b"VIDEO";
    let slots = [
        (MODE_X0, DOC, CameraMode::DocumentVision),
        (MODE_X1, PHO, CameraMode::Photo),
        (MODE_X2, VID, CameraMode::Video),
    ];
    for (x0, label, mode) in slots {
        let rect = Rect::new(x0, MODE_STRIP_Y0 + 6, MODE_SLOT_W, 48);
        if !rect.contains(x, y) {
            continue;
        }
        let selected = camera.mode == mode;
        let fill = if selected { chrome::tint(chrome::AMBER, 14) } else { chrome::BODY };
        let border = if selected { chrome::tint(chrome::AMBER, 60) } else { chrome::BORDER };
        let ink = if selected { chrome::AMBER } else { chrome::TEXT_MUTED };
        let c = std::design::primitives::draw_rounded_rect(
            x,
            y,
            rect,
            TOP_PILL_RADIUS,
            fill,
            Some(border),
            1,
        )?;
        return Some(top_row_label(x, y, rect, label, ink, fill).unwrap_or(c));
    }
    None
}

#[inline(never)]
fn get_camera_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    // 0. Status Band (y < STATUS_BAR_HEIGHT)
    if y < STATUS_BAR_HEIGHT {
        let band = interos_std::design::components::StatusBand::new(W);
        let clock = if camera.clock_ok { Some(&camera.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &interos_std::design::components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }

    // 1. Status Bar & Top Controls (y: 0..112)
    if y < TOP_BAND_Y1 {
        if y >= TOP_BAND_Y1 - 2 {
            return chrome::BORDER;
        }

        // Back control: the SDK's 48x48 back button on the chassis.
        if let Some(c) = std::design::back_button::pixel(
            x,
            y,
            chrome::TEXT,
            chrome::BORDER_STRONG,
            chrome::SURFACE,
        ) {
            return c;
        }

        // Flash / torch pill (116x48px)
        if let Some((fill, on_border)) = top_pill_body(x, y, FLASH_RECT) {
            let is_lit = camera.flash_available && camera.flash == FlashMode::On;
            let icon_color = if is_lit {
                chrome::AMBER
            } else if !camera.flash_available || camera.flash == FlashMode::Off {
                chrome::TEXT_MUTED
            } else {
                chrome::SKY // Auto
            };

            if test_lightning(x, y, FLASH_ICON_CX, FLASH_ICON_CY) {
                return icon_color;
            }

            let label: &[u8] = if !camera.flash_available {
                b"OFF"
            } else {
                camera.flash.pill_label()
            };
            if let Some(c) = flash_label(x, y, label, icon_color, fill) {
                return c;
            }

            if on_border {
                return if is_lit {
                    chrome::AMBER
                } else if camera.flash_available {
                    chrome::tint(chrome::SKY, 60)
                } else {
                    chrome::BORDER
                };
            }
            return fill;
        }

        // Capture geometry readout (150x48px)
        if let Some((_fill, on_border)) = top_pill_body(x, y, FRAME_RECT) {
            let (label, label_len) = camera.frame_readout();
            let label = &label[..label_len];
            let color = if camera.status == CameraStatus::AuthorityRefused {
                chrome::REFUSED
            } else {
                chrome::TEXT
            };
            if let Some(c) = top_row_label(x, y, FRAME_RECT, label, color, chrome::SURFACE) {
                return c;
            }
            if on_border {
                return chrome::BORDER;
            }
            return chrome::SURFACE;
        }

        // ISP filter pill (170x48px): an informational control, so sky.
        if let Some((fill, on_border)) = top_pill_body(x, y, FILTER_RECT) {
            let filter_name: &[u8] = match camera.filter {
                CameraFilter::Normal => b"NORMAL",
                CameraFilter::Vivid => b"VIVID",
                CameraFilter::Warm => b"WARM",
                CameraFilter::Cool => b"COOL",
                CameraFilter::Mono => b"MONO",
                CameraFilter::Vintage => b"VINTAGE",
            };
            if let Some(c) = top_row_label(x, y, FILTER_RECT, filter_name, chrome::SKY, fill) {
                return c;
            }
            if on_border {
                return chrome::tint(chrome::SKY, 60);
            }
            return fill;
        }

        // Capture counter badge (156x48px)
        if let Some((_fill, on_border)) = top_pill_body(x, y, SHOTS_RECT) {
            let label = &camera.shots_label[..camera.shots_label_len];
            if let Some(c) = top_row_label(x, y, SHOTS_RECT, label, chrome::TEXT, chrome::SURFACE) {
                return c;
            }
            if on_border {
                return chrome::BORDER;
            }
            return chrome::SURFACE;
        }

        return chrome::BODY;
    }

    // 2. The settings surface (when open)
    if camera.screen == CameraScreen::Settings && SHEET_RECT.contains(x, y) {
        return settings_pixel(x, y, camera);
    }

    // 3. Viewfinder Canvas (y: VIEWFINDER_Y0..VIEWFINDER_Y1, inside gutters)
    if (VIEWFINDER_Y0..VIEWFINDER_Y1).contains(&y) && x >= VIEWFINDER_X0 && x < VIEWFINDER_X1 {
        let vf = Rect::new(
            VIEWFINDER_X0,
            VIEWFINDER_Y0,
            VIEWFINDER_W,
            VIEWFINDER_Y1 - VIEWFINDER_Y0,
        );
        if let Some(c) = std::design::primitives::draw_rounded_rect(
            x,
            y,
            vf,
            R_CARD,
            chrome::WELL,
            Some(chrome::BORDER_STRONG),
            1,
        ) {
            // Dynamic aspect ratio framing bounds
            let (active_y0, active_h) = match camera.aspect_ratio {
                CameraAspectRatio::Ratio16_9 => (VIEWFINDER_Y0, 1000),
                CameraAspectRatio::Ratio4_3 => (VIEWFINDER_Y0 + 47, 906),
                CameraAspectRatio::Square => (VIEWFINDER_Y0 + 160, 680),
            };
            let active_y1 = active_y0 + active_h;

            // The matte outside the active frame is darker. The header pills
            // are painted on top of it so a 4:3 or 1:1 mask cannot eat them.
            if y < active_y0 || y >= active_y1 {
                if let Some(c) = viewfinder_header_pixel(x, y, camera) {
                    return c;
                }
                return chrome::LETTERBOX;
            }

            let rel_frame_y = y - active_y0;
            let rel_frame_x = x - VIEWFINDER_X0;

            // The pills float above the frame, including its edge hairline.
            if let Some(c) = viewfinder_header_pixel(x, y, camera) {
                return c;
            }

            // The active frame's edge: a hairline that separates it from the matte.
            if y == active_y0
                || y == active_y1 - 1
                || x == VIEWFINDER_X0
                || x == VIEWFINDER_X1 - 1
            {
                return chrome::BORDER_STRONG;
            }

            // 3.1 3x3 Rule-of-Thirds grid inside the active frame
            if camera.guides {
                let is_third_x =
                    rel_frame_x == VIEWFINDER_W / 3 || rel_frame_x == (VIEWFINDER_W * 2) / 3;
                let is_third_y =
                    rel_frame_y == active_h / 3 || rel_frame_y == (active_h * 2) / 3;
                if is_third_x || is_third_y {
                    return grid_ink();
                }

                // Center focus reticle, amber like the shutter
                let ret_cx = VIEWFINDER_W / 2;
                let ret_cy = active_h / 2;
                let dx = (rel_frame_x as isize - ret_cx as isize).abs();
                let dy = (rel_frame_y as isize - ret_cy as isize).abs();
                let is_reticle = (dx <= RETICLE_ARM as isize && dy <= RETICLE_ARM as isize)
                    && ((dx >= 36 && dy <= 12) || (dy >= 36 && dx <= 12));
                let is_crosshair = (dx <= 4 && dy <= 1) || (dy <= 4 && dx <= 1);
                if is_reticle || is_crosshair {
                    return chrome::AMBER;
                }
            }

            // 3.2 Self-Timer Countdown Readout
            if camera.countdown_ticks > 0 {
                let center_x: usize = VIEWFINDER_X0 + VIEWFINDER_W / 2;
                let center_y: usize = active_y0 + active_h / 2;
                let c_dx = x as isize - center_x as isize;
                let c_dy = y as isize - center_y as isize;
                let dist_sq = (c_dx * c_dx + c_dy * c_dy) as usize;
                if dist_sq <= 52 * 52 && dist_sq >= 44 * 44 {
                    return chrome::AMBER;
                }
                if dist_sq < 44 * 44 {
                    let sec = ((camera.countdown_ticks + 9) / 10).clamp(1, 9) as u8;
                    let digit = [b'0' + sec];
                    let lw = std::design::font::measure(&digit, typography::BODY_SCALE);
                    let lh = std::design::font::line_height(typography::BODY_SCALE);
                    let lx = center_x.saturating_sub(lw / 2);
                    let ly = center_y.saturating_sub(lh / 2);
                    if let Some(tc) = test_text(
                        x,
                        y,
                        lx,
                        ly,
                        typography::BODY_SCALE,
                        &digit,
                        64,
                        chrome::AMBER,
                        chrome::SURFACE,
                    ) {
                        return tc;
                    }
                    return chrome::SURFACE;
                }
            }

            // 3.3 Video mode indicator / readout. The readout stays 00:00:
            // this platform exposes no video stream, so nothing is invented.
            if camera.mode == CameraMode::Video && camera.countdown_ticks == 0 {
                let rec_rect =
                    Rect::new(VIEWFINDER_X0 + VIEWFINDER_W / 2 - 60, active_y0 + 16, 120, 36);
                if rec_rect.contains(x, y) {
                    if let Some(rc) = std::design::primitives::draw_rounded_rect(
                        x,
                        y,
                        rec_rect,
                        radius::CHIP,
                        chrome::SURFACE,
                        Some(chrome::RECORD),
                        1,
                    ) {
                        let dot_cx = rec_rect.x + 16;
                        let dot_cy = rec_rect.y + rec_rect.h / 2;
                        let dot_dx = x as isize - dot_cx as isize;
                        let dot_dy = y as isize - dot_cy as isize;
                        if (dot_dx * dot_dx + dot_dy * dot_dy) <= 16 {
                            return chrome::RECORD;
                        }
                        let text = b"00:00";
                        let lh = std::design::font::line_height(typography::BODY_SCALE);
                        let lx = rec_rect.x + 32;
                        let ly = rec_rect.y + (rec_rect.h.saturating_sub(lh)) / 2;
                        if let Some(tc) = test_text(
                            x,
                            y,
                            lx,
                            ly,
                            typography::BODY_SCALE,
                            text,
                            rec_rect.w - 32,
                            chrome::TEXT,
                            chrome::SURFACE,
                        ) {
                            return tc;
                        }
                        return rc;
                    }
                }
            }

            // 3.4 The platform's verdict, as a status card with a status dot.
            // WHY the strings come from the model: this demo holds no camera
            // authority, so the card states the platform's own answer. It is
            // not a live image, not sensor telemetry and not a capture.
            if camera.total_photos == 0 && camera.countdown_ticks == 0 {
                let abs_mid = active_y0 + active_h / 2;
                let card = Rect::new(
                    VIEWFINDER_X0 + (VIEWFINDER_W - STATUS_CARD_W) / 2,
                    abs_mid - STATUS_CARD_H / 2,
                    STATUS_CARD_W,
                    STATUS_CARD_H,
                );
                let ink = status_ink(camera.status);
                if card.contains(x, y) {
                    if let Some(card_c) = std::design::primitives::draw_rounded_rect(
                        x,
                        y,
                        card,
                        R_CARD,
                        chrome::SURFACE,
                        Some(chrome::tint(ink, 45)),
                        1,
                    ) {
                        // Icon-like status dot with a quiet halo.
                        let dot_cx = card.x + 30;
                        let dot_cy = abs_mid - 19;
                        let dot_dx = x as isize - dot_cx as isize;
                        let dot_dy = y as isize - dot_cy as isize;
                        let dot_r2 = dot_dx * dot_dx + dot_dy * dot_dy;
                        if dot_r2 <= 121 {
                            if dot_r2 <= 36 {
                                return ink;
                            }
                            return chrome::tint(ink, 26);
                        }

                        let text_x = card.x + 64;
                        if y < abs_mid + 15 {
                            if let Some(tc) = test_text(
                                x,
                                y,
                                text_x,
                                abs_mid.saturating_sub(35),
                                typography::BODY_SCALE,
                                &camera.head[..camera.head_len],
                                STATUS_TEXT_W,
                                chrome::TEXT,
                                chrome::SURFACE,
                            ) {
                                return tc;
                            }
                        } else if y >= abs_mid + 20 {
                            if let Some(tc) = test_text(
                                x,
                                y,
                                text_x,
                                abs_mid + 22,
                                typography::CAPTION_SCALE,
                                &camera.sub[..camera.sub_len],
                                STATUS_TEXT_W,
                                chrome::TEXT_MUTED,
                                chrome::SURFACE,
                            ) {
                                return tc;
                            }
                        }
                        return card_c;
                    }
                }
            }

            return c;
        }
    }

    // 4. Mode Selector Carousel (three 216px slots, 60px tall, >=48px touch)
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
        if let Some(c) = mode_strip_pixel(x, y, camera) {
            return c;
        }
        return chrome::BODY;
    }

    // 5. Bottom Shutter Dock & Hardware Controls (y: DOCK_Y0..DOCK_Y1)
    if (DOCK_Y0..DOCK_Y1 - 30).contains(&y) {
        // 5.0 What the last press of the shutter answered, in the platform's
        // own words, in the strip above the ring. Painted first because the
        // caption is the answer the control owes the user.
        if let Some(c) = camera.shutter_notice_pixel(x, y, camera.shutter_notice_ink(), chrome::BODY) {
            return c;
        }

        // 5.1 Bottom-left Gallery Thumbnail Disc (radius 32, diameter 64px >= 48px)
        if side_button_hit(x, y, PHOTOS_CX) {
            let dx = x as isize - PHOTOS_CX as isize;
            let dy = y as isize - SHUTTER_CY as isize;
            let r2 = (dx * dx + dy * dy) as usize;
            let disc_r = 32usize;
            if r2 <= disc_r * disc_r {
                // Crisp outer ring
                if r2 >= (disc_r - 2) * (disc_r - 2) {
                    return chrome::BORDER_STRONG;
                }
                // Live preview thumbnail of recently captured photo
                if camera.has_thumbnail {
                    let tx = (((dx + disc_r as isize) * THUMBNAIL_DIM as isize) / (disc_r as isize * 2))
                        .clamp(0, (THUMBNAIL_DIM - 1) as isize) as usize;
                    let ty = (((dy + disc_r as isize) * THUMBNAIL_DIM as isize) / (disc_r as isize * 2))
                        .clamp(0, (THUMBNAIL_DIM - 1) as isize) as usize;
                    let idx = (ty * THUMBNAIL_DIM + tx) * 3;
                    let r = camera.thumbnail[idx] as u32;
                    let g = camera.thumbnail[idx + 1] as u32;
                    let b = camera.thumbnail[idx + 2] as u32;
                    return (0xFF << 24) | (r << 16) | (g << 8) | b;
                }
                // Fallback landscape pictogram
                if test_gallery_icon(x, y, PHOTOS_CX, SHUTTER_CY) {
                    return chrome::SKY;
                }
                return chrome::SURFACE;
            }
        }

        // 5.2 Center Shutter Button (r=48 core, outer ring 64px)
        let dx = x as isize - SHUTTER_CX as isize;
        let dy = y as isize - SHUTTER_CY as isize;
        let dist_sq = dx * dx + dy * dy;
        let refused = camera.status == CameraStatus::AuthorityRefused;

        let outer = (SHUTTER_RING_OUTER * SHUTTER_RING_OUTER) as isize;
        let inner = (SHUTTER_RING_INNER * SHUTTER_RING_INNER) as isize;
        let core = (SHUTTER_R_CORE * SHUTTER_R_CORE) as isize;

        // Outer ring: white, amber on feedback, grey when refused.
        if dist_sq <= outer && dist_sq >= inner {
            return if refused {
                chrome::DISABLED
            } else if camera.shutter_feedback_ticks > 0 {
                chrome::AMBER
            } else {
                chrome::TEXT
            };
        }
        // Concentric track gap: the chassis shows through.
        if dist_sq < inner && dist_sq >= core {
            return chrome::BODY;
        }
        // Inner tactile shutter core (r=48)
        if dist_sq < core {
            if refused {
                // A red slash: the control is present, the authority is not.
                if (dx + dy).abs() <= 2 && dx.abs() <= 34 {
                    return chrome::REFUSED;
                }
                return chrome::SURFACE;
            }
            return match camera.mode {
                CameraMode::Video => chrome::RECORD,
                // The shutter is the primary control, so it carries the amber.
                CameraMode::Photo | CameraMode::DocumentVision => chrome::AMBER,
            };
        }

        // 5.3 Bottom-right Settings Button (radius 32, diameter 64px >= 48px)
        if side_button_hit(x, y, SETTINGS_CX) {
            let dx = x as isize - SETTINGS_CX as isize;
            let dy = y as isize - SHUTTER_CY as isize;
            let r2 = (dx * dx + dy * dy) as usize;
            let disc_r = 32usize;
            if r2 <= disc_r * disc_r {
                if test_settings_icon(x, y, SETTINGS_CX, SHUTTER_CY) {
                    return chrome::SKY;
                }
                if r2 >= (disc_r - 2) * (disc_r - 2) {
                    return chrome::BORDER_STRONG;
                }
                return chrome::SURFACE;
            }
        }

        return chrome::BODY;
    }

    // Home gesture bar at bottom
    if HOME_PILL.contains(x, y) {
        return chrome::TEXT_DIM;
    }

    chrome::BODY
}

/// The settings surface: a dark sheet with amber-accented rows.
fn settings_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    if let Some(c) = std::design::primitives::draw_rounded_rect(
        x,
        y,
        SHEET_RECT,
        radius::SHEET,
        chrome::SURFACE,
        Some(chrome::BORDER_STRONG),
        1,
    ) {
        // Amber accent bar beside the title.
        if x >= SHEET_ROW_X0
            && x < SHEET_ROW_X0 + 4
            && y >= SHEET_TITLE_Y0 + 4
            && y < SHEET_TITLE_Y0 + 28
        {
            return chrome::AMBER;
        }

        // Title, on the Body scale.
        if y >= SHEET_TITLE_Y0 && y < SHEET_TITLE_Y0 + 36 {
            if let Some(tc) = test_text(
                x,
                y,
                SHEET_TEXT_X,
                SHEET_TITLE_Y0,
                typography::BODY_SCALE,
                b"CAMERA SETTINGS",
                SHEET_TEXT_W,
                chrome::TEXT,
                chrome::SURFACE,
            ) {
                return tc;
            }
        }

        // Hairline between the title and the first row.
        if y >= SHEET_ROW_Y0 - 6 && y < SHEET_ROW_Y0 - 4 {
            if x >= SHEET_ROW_X0 && x < SHEET_ROW_X0 + SHEET_ROW_W {
                return chrome::BORDER;
            }
        }

        // The rows (height 96px >= 48px)
        for (index, row) in SETTINGS_ROW_ORDER.iter().enumerate() {
            let row_y = SHEET_ROW_Y0 + index * SHEET_ROW_H;
            if y < row_y || y >= row_y + SHEET_ROW_H {
                continue;
            }
            let plate = std::design::primitives::draw_rounded_rect(
                x,
                y,
                std::design::primitives::Rect::new(
                    SHEET_ROW_X0,
                    row_y,
                    SHEET_ROW_W,
                    SHEET_ROW_H - SHEET_ROW_GAP,
                ),
                radius::CHIP,
                chrome::SURFACE_HIGH,
                Some(chrome::BORDER),
                1,
            );
            let title = camera.settings_row_title(*row);
            // WHY the bands are the line boxes themselves: the title and the
            // value are body-scale runs (32 px) and the note is a caption
            // (22 px), so a 34/60 split clipped the value's descenders and let
            // the note start inside the value's box.
            if y < row_y + 34 {
                let title_scale = if std::design::font::measure(title, 2) <= SHEET_TEXT_W { 2 } else { 1 };
                if let Some(tc) = test_text(
                    x,
                    y,
                    SHEET_TEXT_X,
                    row_y + 2,
                    title_scale,
                    title,
                    SHEET_TEXT_W,
                    chrome::TEXT,
                    chrome::SURFACE_HIGH,
                ) {
                    return tc;
                }
            } else if y < row_y + 66 {
                let len = camera.settings_value_lens[index];
                let value = &camera.settings_values[index][..len];
                let val_scale = if std::design::font::measure(value, 2) <= SHEET_TEXT_W { 2 } else { 1 };
                if let Some(tc) = test_text(
                    x,
                    y,
                    SHEET_TEXT_X,
                    row_y + 34,
                    val_scale,
                    value,
                    SHEET_TEXT_W,
                    chrome::SKY,
                    chrome::SURFACE_HIGH,
                ) {
                    return tc;
                }
            } else {
                let note = camera.settings_row_note(*row);
                if !note.is_empty() {
                    if let Some(tc) = test_text(
                        x,
                        y,
                        SHEET_TEXT_X,
                        row_y + 66,
                        typography::CAPTION_SCALE,
                        note,
                        SHEET_TEXT_W,
                        chrome::TEXT_MUTED,
                        chrome::SURFACE_HIGH,
                    ) {
                        return tc;
                    }
                }
            }
            if let Some(plate) = plate {
                return plate;
            }
        }
        return c;
    }
    chrome::BODY
}

fn render_camera_ui(surface: u32, camera: &CameraApp) {
    render_camera_band(surface, camera, 0, H);
}

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
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    TopRow,
    Viewfinder,
    Dock,
    All,
}

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
    None,
    Redraw(Dirty),
    Leave(&'static str),
}

fn exit_to_launcher(surface: u32) -> ! {
    if surface > 0 {
        let _ = std::surface_destroy(surface);
    }
    std::exit_with_code(0);
}

fn open_photo_library(camera: &CameraApp) {
    match camera.open_photo_library() {
        Ok(task_id) => {
            let mut buf = [0u8; 96];
            let prefix = b"[CAMERA APP] Gallery service active (PID ";
            let p = prefix.len().min(buf.len());
            buf[..p].copy_from_slice(&prefix[..p]);
            let mut n = p + camera::append_decimal(task_id as u32, &mut buf[p..]);
            if n < buf.len() {
                buf[n] = b')';
                n += 1;
            }
            if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                std::log(msg).ok();
            }
        }
        Err(camera::ERR_NO_PHOTO) => {
            std::log("[CAMERA APP] No photo captured yet; launching Gallery app directly.").ok();
            if let Ok(task_id) = std::service_package_launch("gallery") {
                let mut buf = [0u8; 96];
                let prefix = b"[CAMERA APP] Gallery service launched via package (PID ";
                let p = prefix.len().min(buf.len());
                buf[..p].copy_from_slice(&prefix[..p]);
                let mut n = p + camera::append_decimal(task_id as u32, &mut buf[p..]);
                if n < buf.len() {
                    buf[n] = b')';
                    n += 1;
                }
                if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                    std::log(msg).ok();
                }
            } else {
                std::log("[CAMERA APP] Gallery service package launch unavailable.").ok();
            }
        }
        Err(code) => {
            log_capture_code(b"[CAMERA APP] Intent dispatch refused, code -", code);
            std::log("[CAMERA APP] Falling back to service package launch for Gallery...").ok();
            if let Ok(task_id) = std::service_package_launch("gallery") {
                let mut buf = [0u8; 96];
                let prefix = b"[CAMERA APP] Gallery service launched via package (PID ";
                let p = prefix.len().min(buf.len());
                buf[..p].copy_from_slice(&prefix[..p]);
                let mut n = p + camera::append_decimal(task_id as u32, &mut buf[p..]);
                if n < buf.len() {
                    buf[n] = b')';
                    n += 1;
                }
                if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
                    std::log(msg).ok();
                }
            }
        }
    }
}

fn log_capture_code(prefix: &[u8], code: i64) {
    let mut buf = [0u8; 96];
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix[..p]);
    let n = p + camera::append_decimal(code.unsigned_abs() as u32, &mut buf[p..]);
    if let Ok(msg) = core::str::from_utf8(&buf[..n]) {
        std::log(msg).ok();
    }
}

fn handle_settings_tap(camera: &mut CameraApp, x: usize, y: usize) -> Tap {
    if !SHEET_RECT.contains(x, y) {
        return Tap::None;
    }
    if y < SHEET_ROW_Y0 {
        return Tap::None;
    }
    let index = (y - SHEET_ROW_Y0) / SHEET_ROW_H;
    if index >= SETTINGS_ROW_ORDER.len() {
        return Tap::None;
    }
    let row = SETTINGS_ROW_ORDER[index];
    match row {
        SettingsRow::Filter => {
            camera.filter = camera.filter.next();
            camera.refresh_settings_values();
            std::log("[CAMERA APP] The image processing for the next capture changed.").ok();
            Tap::Redraw(Dirty::All)
        }
        SettingsRow::Flash => {
            match camera.toggle_flash() {
                Some(on) => {
                    std::log(if on {
                        "[CAMERA APP] Flash mode toggled: ON."
                    } else {
                        "[CAMERA APP] Flash mode toggled: OFF."
                    })
                    .ok();
                }
                None => {
                    std::log("[CAMERA APP] Flash unavailable: hardware backend refused request; flash is off.")
                        .ok();
                }
            }
            camera.refresh_settings_values();
            Tap::Redraw(Dirty::All)
        }
        SettingsRow::Guides => {
            camera.guides = !camera.guides;
            camera.refresh_settings_values();
            std::log(if camera.guides {
                "[CAMERA APP] 3x3 Rule-of-Thirds framing grid enabled."
            } else {
                "[CAMERA APP] Framing grid disabled."
            })
            .ok();
            Tap::Redraw(Dirty::Viewfinder)
        }
        SettingsRow::OpenLibrary => {
            open_photo_library(camera);
            Tap::Redraw(Dirty::Viewfinder)
        }
        SettingsRow::Close => {
            camera.screen = CameraScreen::Viewfinder;
            Tap::Redraw(Dirty::Viewfinder)
        }
        SettingsRow::Format
        | SettingsRow::Photos
        | SettingsRow::LastPhoto
        | SettingsRow::LastCapture
        => {
            Tap::None
        }
    }
}

fn handle_camera_touch(camera: &mut CameraApp, x: u32, y: u32) -> Tap {
    let x = x as usize;
    let y = y as usize;

    // 1. Back control or the home gesture strip: exit to the Launcher
    if std::design::back_button::contains(x, y) || BACK_RECT.contains(x, y) || HOME_HIT.contains(x, y) {
        if camera.screen == CameraScreen::Settings {
            camera.screen = CameraScreen::Viewfinder;
            return Tap::Redraw(Dirty::Viewfinder);
        }
        return Tap::Leave("[CAMERA] Back control or home gesture pressed. Exiting to launcher.");
    }

    // 2. The settings surface, when it is open
    if camera.screen == CameraScreen::Settings {
        return handle_settings_tap(camera, x, y);
    }

    // 3. Flash mode toggle pill (Auto -> On -> Off -> Auto)
    if FLASH_RECT.contains(x, y) {
        match camera.toggle_flash() {
            Some(on) => {
                std::log(if on {
                    "[CAMERA APP] Flash mode toggled: ON."
                } else {
                    "[CAMERA APP] Flash mode toggled: OFF."
                })
                .ok();
            }
            None => {
                std::log("[CAMERA APP] Flash unavailable: hardware backend refused request; flash is off.")
                    .ok();
            }
        }
        let _ = std::audio_haptic_click();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 4. ISP filter toggle pill (Normal -> Vivid -> Warm -> Cool -> Mono -> Vintage)
    if FILTER_RECT.contains(x, y) {
        camera.filter = camera.filter.next();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::TopRow);
    }

    // 5. Viewfinder header overlays:
    // 5.1 Aspect ratio toggle pill (4:3 -> 16:9 -> 1:1 -> 4:3)
    if camera.screen == CameraScreen::Viewfinder && ASPECT_RECT.contains(x, y) {
        camera.aspect_ratio = camera.aspect_ratio.toggle();
        std::audio_haptic_click().ok();
        std::log("[CAMERA APP] Aspect ratio switched.").ok();
        return Tap::Redraw(Dirty::All);
    }

    // 5.2 Rule-of-Thirds Grid toggle pill
    if camera.screen == CameraScreen::Viewfinder && GRID_RECT.contains(x, y) {
        camera.guides = !camera.guides;
        camera.refresh_settings_values();
        std::audio_haptic_click().ok();
        std::log(if camera.guides {
            "[CAMERA APP] 3x3 Rule-of-Thirds framing grid enabled."
        } else {
            "[CAMERA APP] Framing grid disabled."
        })
        .ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 5.3 Self-Timer toggle pill (Off -> 3s -> 10s -> Off)
    if camera.screen == CameraScreen::Viewfinder && TIMER_RECT.contains(x, y) {
        camera.timer_mode = camera.timer_mode.next();
        std::audio_haptic_click().ok();
        std::log("[CAMERA APP] Self-timer mode switched.").ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 6. Mode Selector Carousel: three equal 216px slots in 24..696 (60px tall, >=48px touch)
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
        if x >= MODE_X0 && x < MODE_X0 + MODE_SLOT_W {
            camera.mode = CameraMode::DocumentVision;
            std::log("[CAMERA APP] Switched to Document Vision mode.").ok();
        } else if x >= MODE_X1 && x < MODE_X1 + MODE_SLOT_W {
            camera.mode = CameraMode::Photo;
            std::log("[CAMERA APP] Switched to Photo mode.").ok();
        } else if x >= MODE_X2 && x < MODE_X2 + MODE_SLOT_W {
            camera.mode = CameraMode::Video;
            std::log("[CAMERA APP] Switched to Video mode.").ok();
        }
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::All);
    }

    // 7. Bottom-left Gallery Thumbnail Disc (radius 32, diameter 64px >= 48px)
    if side_button_hit(x, y, PHOTOS_CX) {
        std::audio_haptic_click().ok();
        open_photo_library(camera);
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 8. Bottom-right Settings Button (radius 32, diameter 64px >= 48px)
    if side_button_hit(x, y, SETTINGS_CX) {
        camera.screen = CameraScreen::Settings;
        camera.refresh_settings_values();
        std::audio_haptic_click().ok();
        return Tap::Redraw(Dirty::Viewfinder);
    }

    // 9. Center Shutter Button (r=48 core, outer ring 64px)
    if shutter_hit(x as isize - SHUTTER_CX as isize, y as isize - SHUTTER_CY as isize) {
        if camera.status == CameraStatus::AuthorityRefused {
            std::log("[CAMERA APP] Shutter unavailable: kernel refused camera authority for this task.").ok();
            // WHY the dock repaints for a press that did nothing: the press
            // has an answer and the user has to be able to read it. Before
            // this the refusal reached the serial log only, which left the
            // control looking broken rather than refused.
            camera.refuse_shutter();
            return Tap::Redraw(Dirty::Dock);
        }

        // If self-timer is enabled, begin countdown
        if camera.timer_mode != TimerMode::Off && camera.countdown_ticks == 0 {
            camera.countdown_ticks = camera.timer_mode.seconds() * 10;
            camera.shutter_feedback_ticks = 4;
            // WHY the caption goes when a countdown starts: the strip under
            // the shutter answers the press that just happened, and a press
            // that is counting down has not been refused.
            camera.clear_shutter_notice();
            std::log("[CAMERA APP] Self-timer countdown initiated.").ok();
            std::audio_haptic_click().ok();
            return Tap::Redraw(Dirty::Viewfinder);
        }

        camera.shutter_feedback_ticks = 4;
        std::log("[CAMERA APP] Shutter haptic feedback pulse dispatched.").ok();
        let outcome = if camera.mode == CameraMode::Video {
            camera.stream_video_frame()
        } else {
            camera.capture_still_photo()
        };
        match outcome {
            CaptureOutcome::Published => {
                std::log("[CAMERA APP] The photo is on the device and the document index describes it.").ok();
            }
            CaptureOutcome::WrittenNotIndexed => {
                std::log("[CAMERA APP] The photo is on the device, but the document index did not describe it.").ok();
            }
            CaptureOutcome::WriteRefused(code) => {
                log_capture_code(
                    b"[CAMERA APP] The frame was captured but the platform refused to write it, code -",
                    code,
                );
            }
            CaptureOutcome::CaptureRefused(code) => {
                log_capture_code(b"[CAMERA APP] Capture refused, code -", code);
            }
            CaptureOutcome::NoVideoStream => {
                std::log("[CAMERA APP] The camera HAL exposes no video stream; this platform cannot deliver a video frame.").ok();
            }
            CaptureOutcome::None => {
                std::log("[CAMERA APP] The shutter produced no frame.").ok();
            }
        }
        camera.refresh_settings_values();
        return Tap::Redraw(Dirty::All);
    }

    Tap::None
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[CAMERA APP (Ring 3)] Camera online.").ok();

    let mut camera = CameraApp::new();
    refresh_clock(&mut camera);
    camera.refresh_shots_label();
    camera.refresh_status_text();

    camera.probe_camera_authority();
    match camera.status {
        CameraStatus::AuthorityRefused => {
            std::log("[CAMERA APP] The platform refused camera authority for this task; the viewfinder says so.").ok();
        }
        CameraStatus::NoSensor => {
            std::log("[CAMERA APP] Camera authority held, but the camera HAL reported that no sensor is bound.").ok();
        }
        CameraStatus::AuthorityHeld => {
            std::log("[CAMERA APP] The platform granted camera authority; capture controls are armed.").ok();
        }
        CameraStatus::Failed => {
            std::log("[CAMERA APP] The camera authority query failed; the viewfinder says so.").ok();
        }
    }

    std::log("[CAMERA APP] Step 1: Allocating application surface (z=200)...").ok();
    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(s) => {
            std::log("[CAMERA APP] Step 2: Surface created successfully.").ok();
            s
        }
        Err(_) => {
            std::log("[CAMERA APP] Surface allocation unavailable; falling back to headless execution.").ok();
            std::exit_with_code(0);
        }
    };

    camera.mode = CameraMode::Photo;
    render_camera_ui(surface, &camera);
    let _ = std::surface_flip();
    std::log("[CAMERA APP] Step 3: Initial frame presented to hardware scanout.").ok();

    loop {
        if let Ok(events) = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 100) {
            let _ = &events;

            if (events & std::EVENT_TIMER_TICK) != 0 {
                let mut needs_redraw = false;

                if camera.shutter_feedback_ticks > 0 {
                    camera.shutter_feedback_ticks = camera.shutter_feedback_ticks.saturating_sub(1);
                    if surface > 0 {
                        render_camera_band(surface, &camera, DOCK_Y0, DOCK_Y1);
                        needs_redraw = true;
                    }
                }

                if camera.countdown_ticks > 0 {
                    camera.countdown_ticks -= 1;
                    if camera.countdown_ticks == 0 {
                        camera.shutter_feedback_ticks = 4;
                        let _ = camera.capture_still_photo();
                        camera.refresh_settings_values();
                        if surface > 0 {
                            render_camera_ui(surface, &camera);
                            let _ = std::surface_flip();
                        }
                    } else if surface > 0 {
                        render_camera_band(surface, &camera, VIEWFINDER_Y0, VIEWFINDER_Y1);
                        needs_redraw = true;
                    }
                }

                if needs_redraw && surface > 0 {
                    let _ = std::surface_flip();
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
                        std::log("[CAMERA APP] Navigation gesture recognized. Returning to launcher.").ok();
                        exit_to_launcher(surface);
                    }
                    if input.kind == 1 {
                        match handle_camera_touch(&mut camera, input.x, input.y) {
                            Tap::Leave(msg) => {
                                std::log(msg).ok();
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
                        Dirty::Dock => render_camera_band(surface, &camera, DOCK_Y0, DOCK_Y1),
                        Dirty::All => render_camera_ui(surface, &camera),
                        Dirty::None => {}
                    }
                    let _ = std::surface_flip();
                }
            }
        }

        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[CAMERA APP] Panic detected! Terminating task...").ok();
    std::exit_with_code(1);
}
