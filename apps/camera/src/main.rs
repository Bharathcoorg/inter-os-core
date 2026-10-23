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
use camera::theme::{THEME, Theme, radius};
use std::design::primitives::Rect;

use interos_std::design::components;
use interos_std::design::outline_text;

use camera::{
    CameraApp, CameraAspectRatio, CameraFilter, CameraMode, CameraScreen, CameraStatus, CaptureOutcome, FlashMode,
    SettingsRow, TimerMode, SETTINGS_ROW_ORDER, THUMBNAIL_DIM, VIEWFINDER_TEXT_W,
};

/// The platform's corner radii, under the short names the renderer uses.
const R_CARD: usize = radius::CARD;

/// The platform's card treatment for the settings sheet.
const SHEET_CARD: components::CardStyle =
    components::card_style(&THEME, components::Elevation::Raised);

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// Ink for a control that is painted but has nothing behind it.
const DISABLED: u32 = THEME.border;

// -----------------------------------------------------------------------------
// Screen bands
// -----------------------------------------------------------------------------
const TOP_BAND_Y0: usize = 0;
const TOP_BAND_Y1: usize = 112;

const VIEWFINDER_Y0: usize = 120;
const VIEWFINDER_Y1: usize = 1120;

/// Inner width of the viewfinder canvas (inset 20px from each edge).
const VIEWFINDER_W: usize = W - 40;

/// Aspect ratio toggle pill inside the viewfinder header (4:3 vs 16:9 vs 1:1).
const ASPECT_RECT: Rect = Rect::new(36, VIEWFINDER_Y0 + 16, 76, 48);
/// Rule-of-Thirds 3x3 framing grid toggle pill inside the viewfinder header.
const GRID_RECT: Rect = Rect::new(124, VIEWFINDER_Y0 + 16, 76, 48);
/// Self-timer mode toggle pill inside the viewfinder header.
const TIMER_RECT: Rect = Rect::new(212, VIEWFINDER_Y0 + 16, 76, 48);

const MODE_STRIP_Y0: usize = 1144;
const MODE_STRIP_Y1: usize = 1204;

const DOCK_Y0: usize = 1210;
const DOCK_Y1: usize = H;

// -----------------------------------------------------------------------------
// Top control row geometry (height 48px, all >= 48px touch affordance)
// -----------------------------------------------------------------------------
const TOP_ROW_Y: usize = 24;
const TOP_ROW_H: usize = 48;

/// Back control, 52x48: comfortably exceeds >= 48px touch target.
const BACK_RECT: Rect = Rect::new(20, TOP_ROW_Y, 52, TOP_ROW_H);
/// Flash / torch pill: 116x48 (>=48px).
const FLASH_RECT: Rect = Rect::new(80, TOP_ROW_Y, 116, TOP_ROW_H);
/// Capture-geometry readout: 150x48. Reports platform geometry.
const FRAME_RECT: Rect = Rect::new(204, TOP_ROW_Y, 150, TOP_ROW_H);
/// ISP filter pill: 170x48 (>=48px).
const FILTER_RECT: Rect = Rect::new(362, TOP_ROW_Y, 170, TOP_ROW_H);
/// Capture counter badge: 156x48. Reports `total_photos`.
const SHOTS_RECT: Rect = Rect::new(540, TOP_ROW_Y, 156, TOP_ROW_H);

/// Corner radius of the top-row pills.
const TOP_PILL_RADIUS: usize = 18;

/// Centre of the flash pictogram inside `FLASH_RECT`.
const FLASH_ICON_CX: usize = FLASH_RECT.x + 24;
const FLASH_ICON_CY: usize = FLASH_RECT.y + FLASH_RECT.h / 2;
/// Text room to the right of the flash pictogram, inside `FLASH_RECT`.
const FLASH_TEXT_X: usize = FLASH_RECT.x + 36;
const FLASH_TEXT_W: usize = FLASH_RECT.w.saturating_sub(44);

/// The system home gesture strip: a 200x6 indicator drawn inside a 240x48 box.
const HOME_HIT: Rect = Rect::new(240, 1392, 240, 48);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

// Shutter button (r=48 core, outer ring 64px)
const SHUTTER_CX: usize = 360;
const SHUTTER_CY: usize = 1310;
const SHUTTER_RING_OUTER: usize = 64;
const SHUTTER_RING_INNER: usize = 56;
const SHUTTER_R_CORE: usize = 48;

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
        if x < x0 || x - x0 >= w || y < y0 || y - y0 >= h {
            return None;
        }
        return std::design::font::test_text_aa(x, y, x0, y0, scale, text, color, bg, false);
    }
    let (bx, bt, bw, bh) = outline_text::run_box(x0, y0, scale, text);
    if x < bx || x - bx >= bw || y < bt || y - bt >= bh {
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
    Some((THEME.surface_container, on_border))
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

/// The ink of the thirds grid.
fn grid_ink() -> u32 {
    Theme::blend(THEME.surface_container_low, THEME.on_surface_variant, 40)
}

/// The ink of the viewfinder's border.
fn viewfinder_border_ink() -> u32 {
    Theme::blend(THEME.surface, THEME.on_surface, 204)
}

#[inline(never)]
fn get_camera_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    // 1. Status Bar & Top Controls (y: 0..112)
    if y < TOP_BAND_Y1 {
        if y >= TOP_BAND_Y1 - 2 {
            return THEME.border;
        }

        // Back control (52x48px)
        if let Some(c) = std::design::back_button::pixel(x, y, THEME.on_surface, THEME.border, THEME.surface_container) {
            return c;
        }

        // Flash / torch pill (116x48px)
        if let Some((fill, on_border)) = top_pill_body(x, y, FLASH_RECT) {
            let is_lit = camera.flash_available && camera.flash == FlashMode::On;
            let icon_color = if is_lit {
                THEME.warning
            } else if !camera.flash_available || camera.flash == FlashMode::Off {
                THEME.on_surface_variant
            } else {
                THEME.primary // Auto
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
                return if is_lit { THEME.warning } else { THEME.border };
            }
            return fill;
        }

        // Capture geometry readout (150x48px)
        if let Some((_fill, on_border)) = top_pill_body(x, y, FRAME_RECT) {
            let (label, label_len) = camera.frame_readout();
            let label = &label[..label_len];
            let color = if camera.status == CameraStatus::AuthorityRefused {
                THEME.error
            } else {
                THEME.primary
            };
            if let Some(c) = top_row_label(x, y, FRAME_RECT, label, color, THEME.surface_container) {
                return c;
            }
            if on_border {
                return DISABLED;
            }
            return THEME.surface_container;
        }

        // ISP filter pill (170x48px)
        if let Some((fill, on_border)) = top_pill_body(x, y, FILTER_RECT) {
            let (filter_name, filter_color) = match camera.filter {
                CameraFilter::Normal => (&b"NORMAL"[..], THEME.primary),
                CameraFilter::Vivid => (&b"VIVID"[..], THEME.error),
                CameraFilter::Warm => (&b"WARM"[..], THEME.warning),
                CameraFilter::Cool => (&b"COOL"[..], THEME.primary),
                CameraFilter::Mono => (&b"MONO"[..], THEME.on_surface_variant),
                CameraFilter::Vintage => (&b"VINTAGE"[..], THEME.warning),
            };
            if let Some(c) = top_row_label(x, y, FILTER_RECT, filter_name, filter_color, fill) {
                return c;
            }
            if on_border {
                return filter_color;
            }
            return fill;
        }

        // Capture counter badge (156x48px)
        if let Some((_fill, on_border)) = top_pill_body(x, y, SHOTS_RECT) {
            let label = &camera.shots_label[..camera.shots_label_len];
            if let Some(c) = top_row_label(x, y, SHOTS_RECT, label, THEME.on_surface, THEME.surface_container) {
                return c;
            }
            if on_border {
                return THEME.border;
            }
            return THEME.surface_container;
        }

        return THEME.surface;
    }

    // 2. The settings surface (when open)
    if camera.screen == CameraScreen::Settings && SHEET_RECT.contains(x, y) {
        return settings_pixel(x, y, camera);
    }

    // 3. Viewfinder Canvas (y: VIEWFINDER_Y0..VIEWFINDER_Y1, x: 20..700)
    if (VIEWFINDER_Y0..VIEWFINDER_Y1).contains(&y) && x >= 20 && x < W - 20 {
        let vf = Rect::new(20, VIEWFINDER_Y0, VIEWFINDER_W, VIEWFINDER_Y1 - VIEWFINDER_Y0);
        if let Some(c) = std::design::primitives::draw_rounded_rect(
            x,
            y,
            vf,
            R_CARD,
            THEME.surface_container_low,
            Some(viewfinder_border_ink()),
            1,
        ) {
            // Viewfinder top overlay pills (always accessible above matte)
            // 3.1 Aspect Ratio toggle pill (76x48px)
            if ASPECT_RECT.contains(x, y) {
                if let Some(pill_c) = std::design::primitives::draw_rounded_rect(x, y, ASPECT_RECT, 14, 0xCC111214, Some(THEME.border), 1) {
                    let lbl = camera.aspect_ratio.label();
                    let scale = 2;
                    let lw = std::design::font::measure(lbl, scale);
                    let lx = ASPECT_RECT.x + (ASPECT_RECT.w.saturating_sub(lw)) / 2;
                    let lh = std::design::font::line_height(scale);
                    let ly = ASPECT_RECT.y + (ASPECT_RECT.h.saturating_sub(lh)) / 2;
                    if let Some(tc) = test_text(x, y, lx, ly, scale, lbl, ASPECT_RECT.w, THEME.primary, 0xCC111214) {
                        return tc;
                    }
                    return pill_c;
                }
            }

            // 3.2 Rule-of-Thirds Grid toggle pill (76x48px)
            if GRID_RECT.contains(x, y) {
                let border_col = if camera.guides { THEME.primary } else { THEME.border };
                if let Some(pill_c) = std::design::primitives::draw_rounded_rect(x, y, GRID_RECT, 14, 0xCC111214, Some(border_col), 1) {
                    let lbl = b"GRID";
                    let scale = 2;
                    let lw = std::design::font::measure(lbl, scale);
                    let lx = GRID_RECT.x + (GRID_RECT.w.saturating_sub(lw)) / 2;
                    let lh = std::design::font::line_height(scale);
                    let ly = GRID_RECT.y + (GRID_RECT.h.saturating_sub(lh)) / 2;
                    let text_color = if camera.guides { THEME.primary } else { THEME.on_surface_variant };
                    if let Some(tc) = test_text(x, y, lx, ly, scale, lbl, GRID_RECT.w, text_color, 0xCC111214) {
                        return tc;
                    }
                    return pill_c;
                }
            }

            // 3.3 Self-Timer toggle pill (76x48px)
            if TIMER_RECT.contains(x, y) {
                let is_active = camera.timer_mode != TimerMode::Off;
                let border_col = if is_active { THEME.warning } else { THEME.border };
                if let Some(pill_c) = std::design::primitives::draw_rounded_rect(x, y, TIMER_RECT, 14, 0xCC111214, Some(border_col), 1) {
                    let lbl = camera.timer_mode.label();
                    let scale = 2;
                    let lw = std::design::font::measure(lbl, scale);
                    let lx = TIMER_RECT.x + (TIMER_RECT.w.saturating_sub(lw)) / 2;
                    let lh = std::design::font::line_height(scale);
                    let ly = TIMER_RECT.y + (TIMER_RECT.h.saturating_sub(lh)) / 2;
                    let text_color = if is_active { THEME.warning } else { THEME.on_surface_variant };
                    if let Some(tc) = test_text(x, y, lx, ly, scale, lbl, TIMER_RECT.w, text_color, 0xCC111214) {
                        return tc;
                    }
                    return pill_c;
                }
            }

            // Dynamic Aspect Ratio framing bounds
            let (active_y0, active_h) = match camera.aspect_ratio {
                CameraAspectRatio::Ratio16_9 => (VIEWFINDER_Y0, 1000),
                CameraAspectRatio::Ratio4_3 => (VIEWFINDER_Y0 + 47, 906),
                CameraAspectRatio::Square => (VIEWFINDER_Y0 + 160, 680),
            };
            let active_y1 = active_y0 + active_h;

            // Translucent dark letterbox bars outside active aspect ratio frame
            if y < active_y0 || y >= active_y1 {
                return 0xEE090A0C;
            }

            let rel_frame_y = y - active_y0;
            let rel_frame_x = x - 20;

            // 3.4 3x3 Rule-of-Thirds Grid framing lines inside active frame
            if camera.guides {
                let is_third_x = rel_frame_x == 226 || rel_frame_x == 453;
                let is_third_y = rel_frame_y == (active_h / 3) || rel_frame_y == ((active_h * 2) / 3);
                if is_third_x || is_third_y {
                    return grid_ink();
                }

                // Center focus reticle
                let ret_cx = 340;
                let ret_cy = active_h / 2;
                let dx = (rel_frame_x as isize - ret_cx as isize).abs();
                let dy = (rel_frame_y as isize - ret_cy as isize).abs();
                let is_reticle = (dx <= RETICLE_ARM as isize && dy <= RETICLE_ARM as isize)
                    && ((dx >= 36 && dy <= 12) || (dy >= 36 && dx <= 12));
                let is_crosshair = (dx <= 4 && dy <= 1) || (dy <= 4 && dx <= 1);
                if is_reticle || is_crosshair {
                    return THEME.warning;
                }
            }

            // 3.5 Self-Timer Countdown Readout (Scale 2 Body badge in center)
            if camera.countdown_ticks > 0 {
                let center_x: usize = 360;
                let center_y: usize = active_y0 + active_h / 2;
                let c_dx = x as isize - center_x as isize;
                let c_dy = y as isize - center_y as isize;
                let dist_sq = (c_dx * c_dx + c_dy * c_dy) as usize;
                if dist_sq <= 52 * 52 && dist_sq >= 44 * 44 {
                    return THEME.warning;
                }
                if dist_sq < 44 * 44 {
                    let sec = ((camera.countdown_ticks + 9) / 10).clamp(1, 9) as u8;
                    let digit = [b'0' + sec];
                    let lw = std::design::font::measure(&digit, 2);
                    let lh = std::design::font::line_height(2);
                    let lx = center_x.saturating_sub(lw / 2);
                    let ly = center_y.saturating_sub(lh / 2);
                    if let Some(tc) = test_text(x, y, lx, ly, 2, &digit, 64, THEME.warning, THEME.surface_container_low) {
                        return tc;
                    }
                    return 0xCC111214;
                }
            }

            // 3.6 Video Mode indicator / timer readout
            if camera.mode == CameraMode::Video && camera.countdown_ticks == 0 {
                let rec_rect = Rect::new(300, active_y0 + 16, 120, 36);
                if rec_rect.contains(x, y) {
                    if let Some(rc) = std::design::primitives::draw_rounded_rect(x, y, rec_rect, 10, 0xCC111214, Some(THEME.error), 1) {
                        let dot_cx = rec_rect.x + 16;
                        let dot_cy = rec_rect.y + rec_rect.h / 2;
                        let dot_dx = x as isize - dot_cx as isize;
                        let dot_dy = y as isize - dot_cy as isize;
                        if (dot_dx * dot_dx + dot_dy * dot_dy) <= 16 {
                            return THEME.error;
                        }
                        let text = b"00:00";
                        let lh = std::design::font::line_height(2);
                        let lx = rec_rect.x + 32;
                        let ly = rec_rect.y + (rec_rect.h.saturating_sub(lh)) / 2;
                        if let Some(tc) = test_text(x, y, lx, ly, 2, text, rec_rect.w - 32, THEME.on_surface, 0xCC111214) {
                            return tc;
                        }
                        return rc;
                    }
                }
            }

            // 3.7 The absence panel (when total_photos == 0 and countdown is idle)
            if camera.total_photos == 0 && camera.countdown_ticks == 0 {
                let abs_mid = active_y0 + active_h / 2;
                if y >= abs_mid.saturating_sub(40) && y < abs_mid + 60 {
                    if y < abs_mid + 15 {
                        if let Some(tc) = test_text(
                            x,
                            y,
                            camera.head_x,
                            abs_mid.saturating_sub(35),
                            2,
                            &camera.head[..camera.head_len],
                            VIEWFINDER_TEXT_W,
                            THEME.on_surface,
                            THEME.surface_container_low,
                        ) {
                            return tc;
                        }
                    } else if y >= abs_mid + 20 {
                        if let Some(tc) = test_text(
                            x,
                            y,
                            camera.sub_x,
                            abs_mid + 22,
                            1,
                            &camera.sub[..camera.sub_len],
                            VIEWFINDER_TEXT_W,
                            THEME.on_surface_variant,
                            THEME.surface_container_low,
                        ) {
                            return tc;
                        }
                    }
                }
            }
            return c;
        }
    }

    // 4. Mode Selector Carousel (y: 1144..1204, 60px tall, >=48px touch)
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
        let y0 = MODE_STRIP_Y0 + ((MODE_STRIP_Y1 - MODE_STRIP_Y0).saturating_sub(lh)) / 2;

        if x < S1_X {
            let is_doc = camera.mode == CameraMode::DocumentVision;
            let doc_color = if is_doc { THEME.on_surface } else { THEME.on_surface_variant };
            let doc_w = std::design::font::measure(DOC, scale);
            let doc_x = S0_X + (S0_W.saturating_sub(doc_w)) / 2;
            if let Some(c) = test_text(x, y, doc_x, y0, scale, DOC, S0_W, doc_color, THEME.surface) {
                return c;
            }
        } else if x < S2_X {
            let is_photo = camera.mode == CameraMode::Photo;
            let photo_color = if is_photo { THEME.on_surface } else { THEME.on_surface_variant };
            let pho_w = std::design::font::measure(PHO, scale);
            let pho_x = S1_X + (S1_W.saturating_sub(pho_w)) / 2;
            if let Some(c) = test_text(x, y, pho_x, y0, scale, PHO, S1_W, photo_color, THEME.surface) {
                return c;
            }
        } else {
            let is_vid = camera.mode == CameraMode::Video;
            let vid_color = if is_vid { THEME.on_surface } else { THEME.on_surface_variant };
            let vid_w = std::design::font::measure(VID, scale);
            let vid_x = S2_X + (S2_W.saturating_sub(vid_w)) / 2;
            if let Some(c) = test_text(x, y, vid_x, y0, scale, VID, S2_W, vid_color, THEME.surface) {
                return c;
            }
        }

        // Active mode indicator underline, 64px centered in active slot
        let is_active_mode = match camera.mode {
            CameraMode::Photo => x >= 328 && x < 392,
            CameraMode::Video => x >= 555 && x < 619,
            CameraMode::DocumentVision => x >= 101 && x < 165,
        };
        if is_active_mode && y >= MODE_STRIP_Y1 - 5 && y < MODE_STRIP_Y1 - 1 {
            return THEME.on_surface;
        }

        return THEME.surface;
    }

    // 5. Bottom Shutter Dock & Hardware Controls (y: DOCK_Y0..DOCK_Y1)
    if (DOCK_Y0..DOCK_Y1 - 30).contains(&y) {
        // 5.1 Bottom-left Gallery Thumbnail Disc (radius 32, diameter 64px >= 48px)
        if side_button_hit(x, y, PHOTOS_CX) {
            let dx = x as isize - PHOTOS_CX as isize;
            let dy = y as isize - SHUTTER_CY as isize;
            let r2 = (dx * dx + dy * dy) as usize;
            let disc_r = 32usize;
            if r2 <= disc_r * disc_r {
                // Crisp outer metallic ring
                if r2 >= (disc_r - 2) * (disc_r - 2) {
                    return THEME.on_surface;
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
                    return THEME.on_surface;
                }
                return THEME.surface_container_high;
            }
        }

        // 5.2 Center Shutter Button (r=48 core, outer ring 64px)
        let dx = x as isize - SHUTTER_CX as isize;
        let dy = y as isize - SHUTTER_CY as isize;
        let dist_sq = dx * dx + dy * dy;

        let outer = (SHUTTER_RING_OUTER * SHUTTER_RING_OUTER) as isize;
        let inner = (SHUTTER_RING_INNER * SHUTTER_RING_INNER) as isize;
        let core = (SHUTTER_R_CORE * SHUTTER_R_CORE) as isize;

        // Outer white metallic ring
        if dist_sq <= outer && dist_sq >= inner {
            return if camera.status == CameraStatus::AuthorityRefused {
                DISABLED
            } else if camera.shutter_feedback_ticks > 0 {
                THEME.primary
            } else {
                THEME.on_surface
            };
        }
        // Concentric track gap
        if dist_sq < inner && dist_sq >= core {
            return THEME.surface;
        }
        // Inner tactile shutter core (r=48)
        if dist_sq < core {
            if camera.status == CameraStatus::AuthorityRefused {
                return THEME.surface_container;
            } else if camera.mode == CameraMode::Video {
                return THEME.error;
            } else if camera.mode == CameraMode::DocumentVision {
                return THEME.primary;
            } else {
                return THEME.on_surface;
            }
        }

        // 5.3 Bottom-right Settings Button (radius 32, diameter 64px >= 48px)
        if side_button_hit(x, y, SETTINGS_CX) {
            let dx = x as isize - SETTINGS_CX as isize;
            let dy = y as isize - SHUTTER_CY as isize;
            let r2 = (dx * dx + dy * dy) as usize;
            let disc_r = 32usize;
            if r2 <= disc_r * disc_r {
                if test_settings_icon(x, y, SETTINGS_CX, SHUTTER_CY) {
                    return THEME.on_surface;
                }
                if r2 >= (disc_r - 2) * (disc_r - 2) {
                    return THEME.border;
                }
                return THEME.surface_container_high;
            }
        }

        return THEME.surface;
    }

    // Home gesture bar at bottom
    if HOME_PILL.contains(x, y) {
        return THEME.on_surface_variant;
    }

    THEME.surface
}

/// The settings surface.
fn settings_pixel(x: usize, y: usize, camera: &CameraApp) -> u32 {
    if let Some(c) = std::design::primitives::draw_rounded_rect(
        x,
        y,
        SHEET_RECT,
        SHEET_CARD.radius,
        SHEET_CARD.fill,
        SHEET_CARD.border,
        1,
    ) {
        // Title upgraded to Scale 2 Body
        if y >= SHEET_TITLE_Y0 && y < SHEET_TITLE_Y0 + 36 {
            if let Some(tc) = test_text(
                x,
                y,
                SHEET_TEXT_X,
                SHEET_TITLE_Y0,
                2,
                b"CAMERA SETTINGS",
                SHEET_TEXT_W,
                THEME.on_surface,
                SHEET_CARD.fill,
            ) {
                return tc;
            }
        }

        // The rows (height 96px >= 48px)
        for (index, row) in SETTINGS_ROW_ORDER.iter().enumerate() {
            let row_y = SHEET_ROW_Y0 + index * SHEET_ROW_H;
            if y < row_y || y >= row_y + SHEET_ROW_H {
                continue;
            }
            let style = components::list_row_style(&THEME, false);
            let plate = std::design::primitives::draw_rounded_rect(
                x,
                y,
                std::design::primitives::Rect::new(
                    SHEET_ROW_X0,
                    row_y,
                    SHEET_ROW_W,
                    SHEET_ROW_H - SHEET_ROW_GAP,
                ),
                style.radius,
                style.fill,
                None,
                0,
            );
            let title = camera.settings_row_title(*row);
            if y < row_y + 34 {
                let title_scale = if std::design::font::measure(title, 2) <= SHEET_TEXT_W { 2 } else { 1 };
                if let Some(tc) =
                    test_text(x, y, SHEET_TEXT_X, row_y + 4, title_scale, title, SHEET_TEXT_W, THEME.on_surface, style.fill)
                {
                    return tc;
                }
            } else if y < row_y + 60 {
                let len = camera.settings_value_lens[index];
                let value = &camera.settings_values[index][..len];
                let val_scale = if std::design::font::measure(value, 2) <= SHEET_TEXT_W { 2 } else { 1 };
                if let Some(tc) =
                    test_text(x, y, SHEET_TEXT_X, row_y + 36, val_scale, value, SHEET_TEXT_W, THEME.primary, style.fill)
                {
                    return tc;
                }
            } else {
                let note = camera.settings_row_note(*row);
                if !note.is_empty() {
                    if let Some(tc) = test_text(
                        x,
                        y,
                        SHEET_TEXT_X,
                        row_y + 64,
                        1,
                        note,
                        SHEET_TEXT_W,
                        THEME.on_surface_variant,
                        style.fill,
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
    THEME.surface
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

    // 6. Mode Selector Carousel: three equal 226px slots in 20..700 (60px tall, >=48px touch)
    if (MODE_STRIP_Y0..MODE_STRIP_Y1).contains(&y) {
        if x >= 20 && x < 246 {
            camera.mode = CameraMode::DocumentVision;
            std::log("[CAMERA APP] Switched to Document Vision mode.").ok();
        } else if x >= 247 && x < 473 {
            camera.mode = CameraMode::Photo;
            std::log("[CAMERA APP] Switched to Photo mode.").ok();
        } else if x >= 474 && x < 700 {
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
            return Tap::None;
        }

        // If self-timer is enabled, begin countdown
        if camera.timer_mode != TimerMode::Off && camera.countdown_ticks == 0 {
            camera.countdown_ticks = camera.timer_mode.seconds() * 10;
            camera.shutter_feedback_ticks = 4;
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
