//! InterOS Native Gallery App (light Photos surface)
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Image-first 3-column thumbnail grid on the canonical 8pt grid:
//!   `3 x 216 cell + 2 x 12 gutter + 2 x 24 margin = 720` exactly
//! - Cells only ever render records from the real `PHOTOS` ledger; with an empty
//!   ledger the band shows the design-system honest empty state, never filler tiles
//! - Full-screen photo inspection view (light themed) driven by a ledger record
//! - Segmented filter tabs (All / Albums / Favorites) and a bottom navigation bar
//! - Every string is measured with `interos_std::design::font::measure` /
//!   `ellipsize` at the coordinates it is drawn at, so text never clips or overlaps
//! - Asynchronous event reactor loop, graceful lifecycle supervision and clean
//!   return to the Launcher

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use std::design::font;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

// -----------------------------------------------------------------------------
// 32-bit ARGB8888 Design Color Tokens (light Photos surface)
// -----------------------------------------------------------------------------
const BG: u32 = 0xFFF7F6F2;
const SURFACE: u32 = 0xFFFFFFFF;
const SOFT: u32 = 0xFFEFEEE9;
const BORDER: u32 = 0xFFE1E0DB;
const TEXT: u32 = 0xFF111214;
const MUTED: u32 = 0xFF777773;
const INVERSE: u32 = 0xFFFFFFFF;

// -----------------------------------------------------------------------------
// Layout constants - single source of truth for rendering *and* hit-testing
// -----------------------------------------------------------------------------
/// Screen margin (8pt grid value).
const MARGIN: usize = 24;
/// Grid gutter between columns/rows. 8px gutters with a 24px margin cannot
/// produce an integer cell on a 720px canvas (720 - 48 - 16 = 656, 656/3 is not
/// integral), so the gutter is the next 8pt-grid value that divides exactly:
/// 3 x 216 + 2 x 12 + 2 x 24 = 648 + 24 + 48 = 720.
const GUTTER: usize = 12;
const CELL: usize = 216;
const COL_X: [usize; 3] = [MARGIN, MARGIN + CELL + GUTTER, MARGIN + 2 * (CELL + GUTTER)];

const HEADER_H: usize = 144;
const TITLE_Y: usize = 44;
const SUBTITLE_Y: usize = 116;

const PILL_Y: usize = 152;
const PILL_H: usize = 48;
const PILL_GAP: usize = 12;
const PILL_PAD: usize = 20;
const PILL_MIN_W: usize = 72;

const GRID_TOP: usize = 216;
const ROW_STRIDE: usize = CELL + GUTTER;
const ROWS: usize = 4;
/// 216 + 4 x 216 + 3 x 12 = 1116: the grid stops clear of the bottom bar.
const GRID_BOTTOM: usize = GRID_TOP + ROWS * CELL + (ROWS - 1) * GUTTER;

const NAV_Y0: usize = 1290;
const NAV_Y1: usize = 1400;
const NAV_ICON_CY: usize = 1330;
const NAV_LABEL_Y: usize = 1356;

const HOME_X0: usize = 240;
const HOME_X1: usize = 480;
const HOME_Y0: usize = 1418;
const HOME_Y1: usize = 1424;

// Inspection view geometry.
const DETAIL_BAR_H: usize = 112;
const DETAIL_VIEW_Y0: usize = 128;
const DETAIL_VIEW_Y1: usize = 800;
const CLOSE_X0: usize = 232;
const CLOSE_Y0: usize = 856;
const CLOSE_W: usize = 256;
const CLOSE_H: usize = 56;

const PILL_LABELS: [&[u8]; 3] = [b"All", b"Albums", b"Favorites"];
const NAV_LABELS: [&[u8]; 3] = [b"Photos", b"Albums", b"Places"];

// -----------------------------------------------------------------------------
// Text rendering helpers - the gate always matches the glyph line box, so
// glyphs are never clipped mid-row.
// -----------------------------------------------------------------------------
#[inline(always)]
fn text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if y < y0 || y >= y0 + font::line_height(scale) {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, false)
}

#[inline(always)]
fn text_bold(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    if y < y0 || y >= y0 + font::line_height(scale) {
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, s, color, bg, true)
}

/// Horizontally centred text inside `[x0, x0 + max_w)`, measured with the real
/// font metrics so it can never run past its container.
#[inline(always)]
fn text_centered(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    max_w: usize,
    scale: usize,
    s: &[u8],
    color: u32,
    bg: u32,
) -> Option<u32> {
    let w = font::measure(s, scale);
    if w > max_w {
        return None;
    }
    text(x, y, x0 + (max_w - w) / 2, y0, scale, s, color, bg)
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

/// The device photo ledger. It is populated only from real media storage, which
/// currently exposes no images, so the ledger is honestly empty.
pub static PHOTOS: [PhotoItem; 0] = [];

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
// Vector Pictogram Engines (one rounded line family, 24-grid geometry)
// -----------------------------------------------------------------------------

/// Empty-state picture frame pictogram (frame + sun + ridge).
fn empty_frame(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dx.abs() <= 60 && dy.abs() <= 48 {
        if dx.abs() >= 57 || dy.abs() >= 45 {
            return true;
        }
        let sun_dx = dx + 22;
        let sun_dy = dy + 16;
        if sun_dx * sun_dx + sun_dy * sun_dy <= 36 {
            return true;
        }
        if dy >= -6 && dy <= 40 {
            let m1 = (dx + 12).abs() * 2 <= (dy + 10) * 3;
            let m2 = (dx - 26).abs() * 2 <= (dy - 4) * 3;
            if m1 || m2 {
                return true;
            }
        }
    }
    false
}

/// Vector icon for the Photos tab (small picture frame).
fn tab_photos_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
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

/// Vector icon for the Albums tab (stacked album cards).
fn tab_albums_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    let front = (dx >= -8 && dx <= 10 && (dy == -6 || dy == 10))
        || (dy >= -6 && dy <= 10 && (dx == -8 || dx == 10));
    let back = (dx >= -12 && dx <= 6 && dy == -10)
        || (dy >= -10 && dy <= 6 && dx == -12);
    front || back
}

/// Vector icon for the Places tab (map pin marker).
fn tab_places_icon(x: usize, y: usize, cx: usize, cy: usize) -> bool {
    let dx = x as isize - cx as isize;
    let dy = y as isize - cy as isize;
    if dy <= 2 {
        let r2 = dx * dx + (dy + 3) * (dy + 3);
        if r2 <= 81 && r2 >= 25 {
            return true;
        }
    } else if dy <= 11 && dx.abs() <= (11 - dy) {
        return true;
    }
    false
}

/// Back chevron stroke (`half` px tall), apex pointing left, centred on (cx, cy).
fn chevron_left(x: usize, y: usize, cx: usize, cy: usize, half: isize) -> bool {
    let dx = (x as isize) - (cx as isize);
    let dy = (y as isize) - (cy as isize);
    let d = dy.abs();
    dx <= 0 && d <= half && (dx + d).abs() <= 2
}

// -----------------------------------------------------------------------------
// Gradient helper (photo records carry their own colours)
// -----------------------------------------------------------------------------
#[inline(always)]
fn blend_argb(top: u32, bottom: u32, t: usize) -> u32 {
    let tr = ((top >> 16) & 0xFF) as isize;
    let tg = ((top >> 8) & 0xFF) as isize;
    let tb = (top & 0xFF) as isize;
    let br = ((bottom >> 16) & 0xFF) as isize;
    let bg = ((bottom >> 8) & 0xFF) as isize;
    let bb = (bottom & 0xFF) as isize;
    let ri = tr + (br - tr) * t as isize / 255;
    let gi = tg + (bg - tg) * t as isize / 255;
    let bi = tb + (bb - tb) * t as isize / 255;
    0xFF000000
        | ((ri.clamp(0, 255) as u32) << 16)
        | ((gi.clamp(0, 255) as u32) << 8)
        | (bi.clamp(0, 255) as u32)
}

// -----------------------------------------------------------------------------
// Segmented filter tabs - geometry derived from the real font metrics so the
// rendered pill and the hit-test are always the same rectangle.
// -----------------------------------------------------------------------------
fn pill_rect(index: usize) -> Rect {
    let mut x = MARGIN;
    for (i, label) in PILL_LABELS.iter().enumerate() {
        let w = (font::measure(label, 2) + PILL_PAD * 2).max(PILL_MIN_W);
        let rect = Rect::new(x, PILL_Y, w, PILL_H);
        if i == index {
            return rect;
        }
        x += w + PILL_GAP;
    }
    Rect::new(0, 0, 0, 0)
}

fn pill_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if y < PILL_Y || y >= PILL_Y + PILL_H {
        return None;
    }
    for (i, label) in PILL_LABELS.iter().enumerate() {
        let rect = pill_rect(i);
        if !rect.contains(x, y) {
            continue;
        }
        let active = app.selected_tab == i;
        let fill = if active { TEXT } else { SOFT };
        let shape = draw_pill(
            x,
            y,
            rect,
            fill,
            if active { None } else { Some(BORDER) },
        )?;
        if shape != fill {
            return Some(shape);
        }
        let lw = font::measure(label, 2);
        let lx = rect.x + rect.w.saturating_sub(lw) / 2;
        if let Some(c) = text(x, y, lx, PILL_Y, 2, label, if active { INVERSE } else { TEXT }, fill)
        {
            return Some(c);
        }
        return Some(fill);
    }
    None
}

// -----------------------------------------------------------------------------
// Grid band
// -----------------------------------------------------------------------------
fn col_of(x: usize) -> Option<usize> {
    for (i, x0) in COL_X.iter().enumerate() {
        if x >= *x0 && x < *x0 + CELL {
            return Some(i);
        }
    }
    None
}

fn row_of(y: usize) -> Option<usize> {
    if y < GRID_TOP || y >= GRID_BOTTOM {
        return None;
    }
    let rel = y - GRID_TOP;
    let row = rel / ROW_STRIDE;
    if row >= ROWS || rel % ROW_STRIDE >= CELL {
        return None;
    }
    Some(row)
}

fn grid_pixel(x: usize, y: usize, _app: &GalleryApp) -> Option<u32> {
    if y < GRID_TOP || y >= GRID_BOTTOM {
        return None;
    }

    // Honest empty state: the ledger holds no media, so nothing is tiled.
    if has_no_media() {
        if empty_frame(x, y, 360, 600) {
            return Some(MUTED);
        }
        let title = b"No images on device";
        if let Some(c) = text_centered(x, y, MARGIN, 690, W - 2 * MARGIN, 2, title, TEXT, BG) {
            return Some(c);
        }
        let note = b"PHOTOS ledger is empty; nothing to show.";
        if let Some(c) = text_centered(x, y, MARGIN, 754, W - 2 * MARGIN, 1, note, MUTED, BG) {
            return Some(c);
        }
        return Some(BG);
    }

    let (Some(col), Some(row)) = (col_of(x), row_of(y)) else {
        return Some(BG);
    };
    let Some(photo) = PHOTOS.get(row * 3 + col) else {
        return Some(BG);
    };
    let x0 = COL_X[col];
    let y0 = GRID_TOP + row * ROW_STRIDE;
    let shape = draw_rounded_rect(
        x,
        y,
        Rect::new(x0, y0, CELL, CELL),
        8,
        0xFF000000,
        Some(BORDER),
        1,
    )?;
    if shape == BORDER {
        return Some(BORDER);
    }
    let t = ((y - y0) * 255 / CELL.max(1)).min(255);
    Some(blend_argb(photo.primary_color, photo.secondary_color, t))
}

// -----------------------------------------------------------------------------
// Bottom navigation
// -----------------------------------------------------------------------------
fn nav_pixel(x: usize, y: usize, app: &GalleryApp) -> Option<u32> {
    if y < NAV_Y0 || y >= NAV_Y1 {
        return None;
    }
    if y == NAV_Y0 {
        return Some(BORDER);
    }
    for (i, label) in NAV_LABELS.iter().enumerate() {
        let box_x0 = i * (W / 3);
        if x < box_x0 || x >= box_x0 + W / 3 {
            continue;
        }
        let cx = box_x0 + W / 6;
        let color = if app.bottom_nav == i { TEXT } else { MUTED };
        let is_icon = match i {
            0 => tab_photos_icon(x, y, cx, NAV_ICON_CY),
            1 => tab_albums_icon(x, y, cx, NAV_ICON_CY),
            _ => tab_places_icon(x, y, cx, NAV_ICON_CY),
        };
        if is_icon {
            return Some(color);
        }
        if let Some(c) = text_centered(
            x,
            y,
            box_x0,
            NAV_LABEL_Y,
            W / 3,
            1,
            label,
            color,
            SURFACE,
        ) {
            return Some(c);
        }
        return Some(SURFACE);
    }
    Some(SURFACE)
}

// -----------------------------------------------------------------------------
// Photo inspection view (light)
// -----------------------------------------------------------------------------
fn detail_pixel(x: usize, y: usize, _app: &GalleryApp, idx: usize) -> u32 {
    let Some(photo) = PHOTOS.get(idx) else {
        return BG;
    };

    // App bar: back control + record title/meta, each measured to fit.
    if y < DETAIL_BAR_H {
        if y == DETAIL_BAR_H - 1 {
            return BORDER;
        }
        let back = Rect::new(MARGIN, 28, 56, 56);
        if let Some(c) = draw_pill(x, y, back, SOFT, Some(BORDER)) {
            if c != SOFT {
                return c;
            }
            if chevron_left(x, y, 52, 56, 12) {
                return TEXT;
            }
            return SOFT;
        }

        let title = photo.title.as_bytes();
        if y >= 28 && y < 28 + font::line_height(2) {
            let mut buf = [0u8; 48];
            let n = font::ellipsize(title, 2, 580, &mut buf);
            if let Some(c) = text(x, y, 96, 28, 2, &buf[..n], TEXT, SURFACE) {
                return c;
            }
        }
        let meta = photo.meta.as_bytes();
        if y >= 80 && y < 80 + font::line_height(1) {
            let mut buf = [0u8; 64];
            let n = font::ellipsize(meta, 1, 580, &mut buf);
            if let Some(c) = text(x, y, 96, 80, 1, &buf[..n], MUTED, SURFACE) {
                return c;
            }
        }
        return SURFACE;
    }

    // Image viewport: the record's own colours, nothing fabricated.
    if y >= DETAIL_VIEW_Y0 && y < DETAIL_VIEW_Y1 && x >= MARGIN && x < W - MARGIN {
        let shape = match draw_rounded_rect(
            x,
            y,
            Rect::new(MARGIN, DETAIL_VIEW_Y0, W - 2 * MARGIN, DETAIL_VIEW_Y1 - DETAIL_VIEW_Y0),
            8,
            0xFF000000,
            Some(BORDER),
            1,
        ) {
            Some(c) => c,
            None => return BG,
        };
        if shape == BORDER {
            return BORDER;
        }
        let t = ((y - DETAIL_VIEW_Y0) * 255 / (DETAIL_VIEW_Y1 - DETAIL_VIEW_Y0)).min(255);
        return blend_argb(photo.primary_color, photo.secondary_color, t);
    }

    // Close control: 256 x 56 pill, label centred on the real metrics.
    let close = Rect::new(CLOSE_X0, CLOSE_Y0, CLOSE_W, CLOSE_H);
    if let Some(c) = draw_pill(x, y, close, SOFT, Some(BORDER)) {
        if c != SOFT {
            return c;
        }
        let label = b"Close";
        let lw = font::measure(label, 2);
        let lx = CLOSE_X0 + (CLOSE_W - lw) / 2;
        let ly = CLOSE_Y0 + (CLOSE_H - font::line_height(2)) / 2;
        if let Some(c) = text(x, y, lx, ly, 2, label, TEXT, SOFT) {
            return c;
        }
        return SOFT;
    }

    // Home gesture pill.
    if y >= HOME_Y0 && y < HOME_Y1 && x >= 260 && x < 460 {
        return MUTED;
    }

    BG
}

// -----------------------------------------------------------------------------
// Main screen
// -----------------------------------------------------------------------------
#[inline(never)]
fn gallery_pixel(x: usize, y: usize, app: &GalleryApp) -> u32 {
    if let Some(idx) = app.inspecting_photo {
        return detail_pixel(x, y, app, idx);
    }

    // 1. Header: screen title + a real ledger summary.
    if y < HEADER_H {
        if let Some(c) = text_bold(x, y, MARGIN, TITLE_Y, 3, b"Photos", TEXT, BG) {
            return c;
        }
        let summary: &[u8] = if has_no_media() {
            b"No items on device"
        } else {
            b"Items on device"
        };
        if let Some(c) = text(x, y, MARGIN, SUBTITLE_Y, 1, summary, MUTED, BG) {
            return c;
        }
        return BG;
    }

    // 2. Segmented filter tabs.
    if let Some(c) = pill_pixel(x, y, app) {
        return c;
    }

    // 3. Thumbnail grid band (or honest empty state).
    if let Some(c) = grid_pixel(x, y, app) {
        return c;
    }

    // 4. Bottom navigation bar.
    if let Some(c) = nav_pixel(x, y, app) {
        return c;
    }

    // 5. Home gesture indicator.
    if y >= HOME_Y0 && y < HOME_Y1 && x >= 260 && x < 460 {
        return MUTED;
    }

    BG
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
            chunk[i] = gallery_pixel(x, y, app);
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

                        // Bottom home pill: closes the inspection view, else exits.
                        if x >= HOME_X0 && x <= HOME_X1 && y >= 1400 {
                            if app.inspecting_photo.is_some() {
                                app.inspecting_photo = None;
                                std::log(0, "[GALLERY] Exited photo inspection view.").ok();
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

                        // Inspection view controls.
                        if app.inspecting_photo.is_some() {
                            let back = Rect::new(MARGIN, 28, 56, 56);
                            let close = Rect::new(CLOSE_X0, CLOSE_Y0, CLOSE_W, CLOSE_H);
                            if back.contains(x, y) || close.contains(x, y) {
                                app.inspecting_photo = None;
                                std::log(0, "[GALLERY] Closed photo inspection view.").ok();
                                if surface > 0 {
                                    render_gallery_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
                                break;
                            }
                            break;
                        }

                        // Segmented filter tabs - the hit rectangle is the drawn pill.
                        if y >= PILL_Y && y < PILL_Y + PILL_H {
                            for i in 0..PILL_LABELS.len() {
                                if pill_rect(i).contains(x, y) {
                                    app.selected_tab = i;
                                    std::log(0, "[GALLERY] Gallery category tab switched.").ok();
                                    if surface > 0 {
                                        render_gallery_ui(surface, &app);
                                        let _ = std::surface_flip(6);
                                    }
                                    break;
                                }
                            }
                            break;
                        }

                        // Thumbnail grid taps.
                        if y >= GRID_TOP && y < GRID_BOTTOM {
                            if has_no_media() {
                                std::log(
                                    0,
                                    "[GALLERY] Photo grid tapped: no media items in storage.",
                                )
                                .ok();
                                break;
                            }
                            if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
                                let p_idx = row * 3 + col;
                                if PHOTOS.get(p_idx).is_some() {
                                    app.inspecting_photo = Some(p_idx);
                                    std::log(
                                        0,
                                        "[GALLERY] Photo tapped: inspection view opened.",
                                    )
                                    .ok();
                                    if surface > 0 {
                                        render_gallery_ui(surface, &app);
                                        let _ = std::surface_flip(6);
                                    }
                                }
                            }
                            break;
                        }

                        // Bottom navigation tabs (three equal, touch-sized columns).
                        if y >= NAV_Y0 && y < NAV_Y1 {
                            let idx = x / (W / 3);
                            if idx < NAV_LABELS.len() {
                                app.bottom_nav = idx;
                                std::audio_haptic_click().ok();
                                if surface > 0 {
                                    render_gallery_ui(surface, &app);
                                    let _ = std::surface_flip(6);
                                }
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
