//! InterOS Native Gallery App (light Photos)
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Light 3-column photo tile layout (procedural gradients, no fake metadata)
//! - Full-screen photo inspection modal (light-themed)
//! - Segmented Filter Tabs (All, Albums, Favorites) & Bottom Navigation Bar
//! - Asynchronous event reactor loop handling touch interactions and home gestures
//! - Graceful lifecycle supervision and clean return to Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (light Photos)
// -----------------------------------------------------------------------------
#[allow(dead_code)]
const BG_DARK: u32 = 0xFFF7F6F2;
#[allow(dead_code)]
const SURFACE_DARK: u32 = 0xFFFFFFFF;
#[allow(dead_code)]
const SURFACE_RAISED: u32 = 0xFFEFEEE9;
#[allow(dead_code)]
const BORDER_SUBTLE: u32 = 0xFFE1E0DB;
#[allow(dead_code)]
const TEXT_PRIMARY: u32 = 0xFF111214;
#[allow(dead_code)]
const TEXT_MUTED: u32 = 0xFF777773;
#[allow(dead_code)]
const ACTIVE_PILL: u32 = 0xFF111214;
#[allow(dead_code)]
const WHITE: u32 = 0xFFFFFFFF;

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

// -----------------------------------------------------------------------------
// Photo Record Definitions
// -----------------------------------------------------------------------------
#[derive(Debug, Clone, Copy)]
pub struct PhotoItem {
    pub title: &'static str,
    pub date: &'static str,
    pub meta: &'static str,
    pub primary_color: u32,
    pub secondary_color: u32,
}

pub static PHOTOS: [PhotoItem; 0] = [];

#[allow(dead_code)]
#[inline]
fn has_no_media() -> bool {
    PHOTOS.is_empty()
}

pub struct GalleryApp {
    pub selected_tab: usize, // 0: All, 1: Albums, 2: Favorites
    pub bottom_nav: usize,   // 0: Photos, 1: Albums, 2: Places
    pub inspecting_photo: Option<usize>,
}

impl GalleryApp {
    pub const fn new() -> Self {
        Self {
            selected_tab: 0,
            bottom_nav: 0,
            inspecting_photo: None,
        }
    }
}

// -----------------------------------------------------------------------------
// Vector Pictogram Engines
// -----------------------------------------------------------------------------
#[allow(dead_code)]
fn test_empty_gallery_frame(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 54 && dy.abs() <= 40 {
        if dx.abs() >= 52 || dy.abs() >= 38 {
            return true;
        }
        let sun_dx = dx + 20;
        let sun_dy = dy + 14;
        if sun_dx * sun_dx + sun_dy * sun_dy <= 49 {
            return true;
        }
        if dy >= -4 && dy <= 34 {
            let m1 = (dx + 10).abs() * 2 <= (dy + 8) * 3;
            let m2 = (dx - 22).abs() * 2 <= (dy - 4) * 3;
            if m1 || m2 {
                return true;
            }
        }
    }
    false
}

/// Vector icon for Photos tab (small picture frame)
fn test_tab_photos_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 12 && dy.abs() <= 10 {
        if dx.abs() >= 10 || dy.abs() >= 8 {
            return true;
        }
        if (dx + 5).abs() <= 2 && (dy + 3).abs() <= 2 {
            return true;
        }
        if dy >= 0 && dy <= 6 && (dx - 2).abs() <= dy {
            return true;
        }
    }
    false
}

/// Vector icon for Albums tab (stacked album cards)
fn test_tab_albums_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let front = (dx >= -8 && dx <= 10 && (dy == -6 || dy == 10))
        || (dy >= -6 && dy <= 10 && (dx == -8 || dx == 10));
    let back = (dx >= -12 && dx <= 6 && dy == -10)
        || (dy >= -10 && dy <= 6 && dx == -12);
    front || back
}

/// Vector icon for Places tab (map pin marker)
fn test_tab_places_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy <= 2 {
        let r2 = dx * dx + (dy + 3) * (dy + 3);
        if r2 <= 9 * 9 && r2 >= 5 * 5 {
            return true;
        }
    } else if dy <= 11 && dx.abs() <= (11 - dy) {
        return true;
    }
    false
}

// -----------------------------------------------------------------------------
// Gradient + pill helpers
// -----------------------------------------------------------------------------
#[inline(always)]
fn blend_argb(top: u32, bottom: u32, t: usize) -> u32 {
    // t in 0..=255
    let tr = ((top >> 16) & 0xFF) as usize;
    let tg = ((top >> 8) & 0xFF) as usize;
    let tb = (top & 0xFF) as usize;
    let br = ((bottom >> 16) & 0xFF) as usize;
    let bg = ((bottom >> 8) & 0xFF) as usize;
    let bb = (bottom & 0xFF) as usize;
    // Linear interpolation with signed math:
    let ri = tr as isize + (br as isize - tr as isize) * t as isize / 255;
    let gi = tg as isize + (bg as isize - tg as isize) * t as isize / 255;
    let bi = tb as isize + (bb as isize - tb as isize) * t as isize / 255;
    0xFF000000
        | ((ri.clamp(0, 255) as u32) << 16)
        | ((gi.clamp(0, 255) as u32) << 8)
        | (bi.clamp(0, 255) as u32)
}

#[inline(always)]
fn tile_palette(tile_index: usize) -> (u32, u32) {
    match tile_index % 6 {
        0 => (0xFFBAE6FD, 0xFF0284C7),
        1 => (0xFFBBF7D0, 0xFF15803D),
        2 => (0xFFFEF3C7, 0xFFD97706),
        3 => (0xFFFCE7F3, 0xFFBE123C),
        4 => (0xFFF1F5F9, 0xFF64748B),
        _ => (0xFFCCFBF1, 0xFF0F766E),
    }
}

#[inline(always)]
fn tile_gradient_color(x: usize, y: usize, tile_index: usize, rel_y: usize, tile_h: usize) -> u32 {
    let (top, bottom) = tile_palette(tile_index);
    let t = (rel_y * 255 / tile_h.max(1)).min(255);
    let base = blend_argb(top, bottom, t);
    // Diverse per-pixel variation computed from (x+y+tile_index).
    let seed = x + y + tile_index * 53;
    let delta = (seed % 17) as isize - 8;
    let r = (((base >> 16) & 0xFF) as isize + delta).clamp(0, 255) as u32;
    let g = (((base >> 8) & 0xFF) as isize + delta).clamp(0, 255) as u32;
    let b = ((base & 0xFF) as isize + delta).clamp(0, 255) as u32;
    0xFF000000 | (r << 16) | (g << 8) | b
}

#[inline(always)]
fn pill_inside(x: usize, y: usize, x0: usize, x1: usize, y0: usize, y1: usize, r: usize) -> bool {
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return false;
    }
    let cx0 = x0 + r;
    let cx1 = x1 - r;
    let cy = (y0 + y1) / 2;
    if x >= cx0 && x < cx1 {
        return true;
    }
    let (ccx, in_cap) = if x < cx0 {
        (cx0, true)
    } else if x >= cx1 {
        (cx1.saturating_sub(1), true)
    } else {
        (0, false)
    };
    if !in_cap {
        return false;
    }
    let dx = x as isize - ccx as isize;
    let dy = y as isize - cy as isize;
    dx * dx + dy * dy <= (r as isize) * (r as isize)
}

#[inline(always)]
fn pill_border_px(
    x: usize,
    y: usize,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    r: usize,
) -> bool {
    if !pill_inside(x, y, x0, x1, y0, y1, r) {
        return false;
    }
    // Inset by 1px: if still inside the shrunken pill, this is interior.
    let ix0 = x0 + 1;
    let ix1 = x1.saturating_sub(1);
    let iy0 = y0 + 1;
    let iy1 = y1.saturating_sub(1);
    let ir = r.saturating_sub(1);
    if ix1 > ix0 && iy1 > iy0 && pill_inside(x, y, ix0, ix1, iy0, iy1, ir) {
        return false;
    }
    true
}

// -----------------------------------------------------------------------------
// Pixel Color Resolution Engine
// -----------------------------------------------------------------------------
#[inline(never)]
fn get_gallery_color(x: usize, y: usize, app: &GalleryApp) -> u32 {
    // Modal full-screen inspection view (light-themed).
    if let Some(idx) = app.inspecting_photo {
        let Some(photo) = PHOTOS.get(idx) else {
            return BG_DARK;
        };

        // Header bar (y: 110..180, x: 30..690)
        if y >= 110 && y < 180 && x >= 30 && x < 690 {
            if y >= 125 && y < 141 {
                if let Some(c) = test_text(x, y, 50, 125, 2, photo.title.as_bytes(), TEXT_PRIMARY)
                {
                    return c;
                }
            }
            if y >= 150 && y < 158 {
                if let Some(c) = test_text(x, y, 50, 150, 1, photo.meta.as_bytes(), TEXT_MUTED) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }

        // Large photo viewport (y: 200..1100, x: 30..690)
        if y >= 200 && y < 1100 && x >= 30 && x < 690 {
            if y == 200 || y == 1099 || x == 30 || x == 689 {
                return BORDER_SUBTLE;
            }
            let rel_y = y - 200;
            let t = (rel_y * 255 / 900).min(255);
            return blend_argb(photo.primary_color, photo.secondary_color, t);
        }

        // Inspection bottom Close pill (x: 230..490, y: 1140..1200)
        if y >= 1140 && y < 1200 && x >= 230 && x < 490 {
            if y == 1140 || y == 1199 || x == 230 || x == 489 {
                return BORDER_SUBTLE;
            }
            if y >= 1162 && y < 1178 {
                if let Some(c) = test_text(x, y, 310, 1162, 2, b"Close", TEXT_PRIMARY) {
                    return c;
                }
            }
            return SURFACE_RAISED;
        }

        if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
            return TEXT_MUTED;
        }
        return BG_DARK;
    }

    // 1. Header (y: 0..140)
    if y < 140 {
        // Time, radio signal and battery are not reported to this app. The
        // shared system status service must provide real readings before these
        // indicators are rendered.
        // Bold title "Photos" scale4 at x48 y76.
        if y >= 76 && y < 108 {
            if let Some(c) = test_text(x, y, 48, 76, 4, b"Photos", TEXT_PRIMARY) {
                return c;
            }
        }

        // Search magnifying-glass centered ~(600,92).
        {
            let dx = x as isize - 600;
            let dy = y as isize - 92;
            let r2 = dx * dx + dy * dy;
            if r2 <= 100 && r2 >= 64 {
                return TEXT_PRIMARY;
            }
            if dx >= 5 && dx <= 14 && dy >= 5 && dy <= 14 && (dx - dy).abs() <= 2 {
                return TEXT_PRIMARY;
            }
        }

        // "+" add glyph at (655,92).
        if (x >= 654 && x < 657 && y >= 84 && y < 100)
            || (x >= 647 && x < 664 && y >= 91 && y < 94)
        {
            return TEXT_PRIMARY;
        }

        // Divider line at y94.
        if y == 94 {
            return BORDER_SUBTLE;
        }

        return BG_DARK;
    }

    // 2. Segmented pills (y: 135..185)
    if y >= 135 && y < 185 {
        // "All" capsule x48..150 solid charcoal rounded.
        if x >= 48 && x < 150 {
            if y >= 152 && y < 168 {
                if let Some(c) = test_text(x, y, 75, 152, 2, b"All", WHITE) {
                    // Only treat as text when the active pill is selected;
                    // otherwise fall through to inactive styling below.
                    if app.selected_tab == 0 {
                        return c;
                    }
                }
                // Inactive text color pass (same geometry, dark text).
                if app.selected_tab != 0 {
                    if let Some(c) = test_text(x, y, 75, 152, 2, b"All", TEXT_PRIMARY) {
                        return c;
                    }
                }
            }
            if pill_border_px(x, y, 48, 150, 135, 185, 20) {
                if app.selected_tab == 0 {
                    return ACTIVE_PILL;
                }
                return BORDER_SUBTLE;
            }
            if pill_inside(x, y, 48, 150, 135, 185, 20) {
                if app.selected_tab == 0 {
                    return ACTIVE_PILL;
                }
                return SURFACE_RAISED;
            }
        }
        // "Albums" x165..285 raised with subtle border.
        if x >= 165 && x < 285 {
            if y >= 152 && y < 168 {
                let active = app.selected_tab == 1;
                let tc = if active { WHITE } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 189, 152, 2, b"Albums", tc) {
                    return c;
                }
            }
            if pill_border_px(x, y, 165, 285, 135, 185, 20) {
                if app.selected_tab == 1 {
                    return ACTIVE_PILL;
                }
                return BORDER_SUBTLE;
            }
            if pill_inside(x, y, 165, 285, 135, 185, 20) {
                if app.selected_tab == 1 {
                    return ACTIVE_PILL;
                }
                return SURFACE_RAISED;
            }
        }
        // "Favorites" x300..430 same inactive style (scale1 to fit).
        if x >= 300 && x < 430 {
            if y >= 156 && y < 164 {
                let active = app.selected_tab == 2;
                let tc = if active { WHITE } else { TEXT_PRIMARY };
                if let Some(c) = test_text(x, y, 329, 156, 1, b"Favorites", tc) {
                    return c;
                }
            }
            if pill_border_px(x, y, 300, 430, 135, 185, 20) {
                if app.selected_tab == 2 {
                    return ACTIVE_PILL;
                }
                return BORDER_SUBTLE;
            }
            if pill_inside(x, y, 300, 430, 135, 185, 20) {
                if app.selected_tab == 2 {
                    return ACTIVE_PILL;
                }
                return SURFACE_RAISED;
            }
        }
        return BG_DARK;
    }

    // 3. 3-column grid (y: 200..1280), no text inside tiles.
    if y >= 200 && y < 1280 {
        const GRID_TOP: usize = 200;
        const TILE_H: usize = 200;
        const STRIDE: usize = 216; // 200 + 16 gap
        const ROWS: usize = 5;
        let col = if (36..236).contains(&x) {
            Some((0usize, 36usize, 236usize))
        } else if (260..460).contains(&x) {
            Some((1usize, 260usize, 460usize))
        } else if (484..684).contains(&x) {
            Some((2usize, 484usize, 684usize))
        } else {
            None
        };
        if let Some((c_idx, x0, x1)) = col {
            let rel_row = y.saturating_sub(GRID_TOP) / STRIDE;
            if rel_row < ROWS {
                let y0 = GRID_TOP + rel_row * STRIDE;
                let y1 = y0 + TILE_H;
                if y >= y0 && y < y1 {
                    let tile_index = rel_row * 3 + c_idx;
                    // Rounded squircle radius 16: cut 16px corners.
                    let r: usize = 16;
                    let in_tl = x < x0 + r && y < y0 + r;
                    let in_tr = x >= x1 - r && y < y0 + r;
                    let in_bl = x < x0 + r && y >= y1 - r;
                    let in_br = x >= x1 - r && y >= y1 - r;
                    if in_tl || in_tr || in_bl || in_br {
                        let (ccx, ccy) = if in_tl {
                            (x0 + r, y0 + r)
                        } else if in_tr {
                            (x1 - 1 - r, y0 + r)
                        } else if in_bl {
                            (x0 + r, y1 - 1 - r)
                        } else {
                            (x1 - 1 - r, y1 - 1 - r)
                        };
                        let dx = x as isize - ccx as isize;
                        let dy = y as isize - ccy as isize;
                        let d2 = dx * dx + dy * dy;
                        if d2 > (r as isize) * (r as isize) {
                            return BG_DARK;
                        }
                        if d2 >= ((r as isize - 1) * (r as isize - 1)) {
                            return BORDER_SUBTLE;
                        }
                    } else if x == x0 || x == x1 - 1 || y == y0 || y == y1 - 1 {
                        return BORDER_SUBTLE;
                    }
                    let rel_y = y - y0;
                    return tile_gradient_color(x, y, tile_index, rel_y, TILE_H);
                }
            }
        }
        return BG_DARK;
    }

    // 4. Bottom navigation bar (y: 1290..1400)
    if y >= 1290 && y < 1400 {
        if y == 1290 {
            return BORDER_SUBTLE;
        }
        let nav: Option<(usize, usize, usize, &'static [u8])> = if (40..220).contains(&x) {
            Some((0, 40, 220, &b"Photos"[..]))
        } else if (270..450).contains(&x) {
            Some((1, 270, 450, &b"Albums"[..]))
        } else if (500..680).contains(&x) {
            Some((2, 500, 680, &b"Places"[..]))
        } else {
            None
        };

        if let Some((idx, x0, x1, label)) = nav {
            let is_sel = app.bottom_nav == idx;
            let icon_color = if is_sel {
                TEXT_PRIMARY
            } else {
                TEXT_MUTED
            };

            let cx = x0 + (x1 - x0) / 2;
            let is_icon = match idx {
                0 => test_tab_photos_icon(x, y, cx, 1340),
                1 => test_tab_albums_icon(x, y, cx, 1340),
                _ => test_tab_places_icon(x, y, cx, 1340),
            };
            if is_icon {
                return icon_color;
            }

            if y >= 1362 && y < 1370 {
                let ox = cx.saturating_sub(label.len() * 4);
                if let Some(c) = test_text(x, y, ox, 1362, 1, label, icon_color) {
                    return c;
                }
            }
        }
        return SURFACE_DARK;
    }

    // 5. Home gesture indicator pill (x: 260..460, y: 1418..1424)
    if y >= 1418 && y < 1424 && x >= 260 && x < 460 {
        return TEXT_MUTED;
    }

    BG_DARK
}

fn render_gallery_ui(surface: u32, app: &GalleryApp) {
    let mut chunk = [0u32; CHUNK];
    let total = W * H;
    let mut offset = 0;
    while offset < total {
        let count = (total - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for i in 0..count {
            chunk[i] = get_gallery_color(x, y, app);
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

// -----------------------------------------------------------------------------
// Entry Point & Event Reactor Loop
// -----------------------------------------------------------------------------
#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[GALLERY (Ring 3)] Gallery online.").ok();

    let mut app = GalleryApp::new();

    // Allocate client surface (slot 6 = FrameBufferDirect, z=200)
    let surface = match std::surface_create(6, W as u32, H as u32, 200) {
        Ok(id) => {
            std::log(0, "[GALLERY] DRM/KMS Client Surface allocated (z=200).").ok();
            id
        }
        Err(_) => 0,
    };

    if surface > 0 {
        render_gallery_ui(surface, &app);
        let _ = std::surface_flip(6);
        std::log(0, "[GALLERY] Gallery UI presented to scanout buffer.").ok();
    }

    // Interactive Event Loop
    loop {
        let interest = std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK;
        if let Ok(triggered) = std::event_poll(interest, 100) {
            if (triggered & std::EVENT_TOUCH_INPUT) != 0 {
                let mut input = std::InputEvent {
                    kind: 0,
                    x: 0,
                    y: 0,
                    dx: 0,
                    dy: 0,
                    scale_milli: 1000,
                };
                while std::input_event_poll(&mut input).is_ok() {
                    // Swipe up home gesture -> Return to launcher
                    if input.kind == 4
                        || input.kind == 6
                        || input.kind == 7
                        || input.kind == 8
                        || input.kind == 10
                    {
                        std::log(
                            0,
                            "[GALLERY] Navigation gesture recognized. Exiting to launcher.",
                        )
                        .ok();
                        if surface > 0 {
                            let _ = std::surface_destroy(6, surface);
                        }
                        std::exit_with_code(0);
                    }

                    if input.kind == 1 {
                        let x = input.x as usize;
                        let y = input.y as usize;

                        // Bottom home pill
                        if x >= 240 && x <= 480 && y >= 1400 {
                            if app.inspecting_photo.is_some() {
                                app.inspecting_photo = None;
                                std::log(0, "[GALLERY] Exited photo inspection modal.").ok();
                                if surface > 0 {
                                    render_gallery_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
                                break;
                            } else {
                                std::log(0, "[GALLERY] Home gesture pressed. Exiting to launcher.")
                                    .ok();
                                if surface > 0 {
                                    let _ = std::surface_destroy(6, surface);
                                }
                                std::exit_with_code(0);
                            }
                        }

                        // If in modal, tapping Close button exits modal
                        if app.inspecting_photo.is_some() {
                            if (1140..1200).contains(&y) && (230..490).contains(&x) {
                                app.inspecting_photo = None;
                                std::log(0, "[GALLERY] Closed photo inspection modal.").ok();
                                if surface > 0 {
                                    render_gallery_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
                                break;
                            }
                        }

                        // Category pills (y: 135..185)
                        if (135..185).contains(&y) {
                            if (48..150).contains(&x) {
                                app.selected_tab = 0;
                            } else if (165..285).contains(&x) {
                                app.selected_tab = 1;
                            } else if (300..430).contains(&x) {
                                app.selected_tab = 2;
                            } else {
                                // Tapped pill row but outside pills: ignore.
                                break;
                            }
                            std::log(0, "[GALLERY] Gallery category tab switched.").ok();
                            if surface > 0 {
                                render_gallery_ui(surface, &app);
                                let _ = std::surface_flip(6);
                            }
                            break;
                        }

                        // Photo grid tile taps (y: 200..1280, 3 cols x 5 rows)
                        if (200..1280).contains(&y) {
                            let col = if (36..236).contains(&x) {
                                Some(0)
                            } else if (260..460).contains(&x) {
                                Some(1)
                            } else if (484..684).contains(&x) {
                                Some(2)
                            } else {
                                None
                            };
                            if let Some(c) = col {
                                let rel_row = y.saturating_sub(200) / 216;
                                // Ignore inter-tile gaps.
                                let y0 = 200 + rel_row * 216;
                                if rel_row < 5 && y >= y0 && y < y0 + 200 {
                                    let p_idx = rel_row * 3 + c;
                                    if PHOTOS.get(p_idx).is_some() {
                                        app.inspecting_photo = Some(p_idx);
                                        std::log(
                                            0,
                                            "[GALLERY] Photo tapped: inspection modal opened.",
                                        )
                                        .ok();
                                        if surface > 0 {
                                            render_gallery_ui(surface, &app);
                                            let _ = std::surface_flip(6);
                                        }
                                        break;
                                    }
                                }
                            }
                        }

                        // Bottom navigation tabs (y: 1290..1400)
                        if (1290..1400).contains(&y) {
                            if (40..220).contains(&x) {
                                app.bottom_nav = 0;
                            } else if (270..450).contains(&x) {
                                app.bottom_nav = 1;
                            } else if (500..680).contains(&x) {
                                app.bottom_nav = 2;
                            } else {
                                break;
                            }
                            std::audio_haptic_click().ok();
                            if surface > 0 {
                                render_gallery_ui(surface, &app);
                                let _ = std::surface_flip(6);
                            }
                            break;
                        }
                    }
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[GALLERY] Panic detected! Terminating task...").ok();
    std::exit();
}
