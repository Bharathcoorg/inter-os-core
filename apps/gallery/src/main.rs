//! InterOS Native Gallery App (light Photos surface)
//!
//! Native Ring-3 graphical application executing in User Mode:
//! - DRM/KMS Client Surface allocation (z=200) and double-buffered scanout presentation
//! - Image-first 3-column thumbnail grid on the canonical 8pt grid:
//!   `3 x 216 cell + 2 x 12 gutter + 2 x 24 margin = 720` exactly
//! - Cells only ever render records from the real `PHOTOS` ledger; with an empty
//!   ledger the band shows the design-system honest empty state, never filler tiles
//! - Full-screen photo inspection view (light themed) driven by a ledger record
//! - Segmented filter chips (All / Albums / Favorites) and a bottom section bar,
//!   both drawn in an explicitly unavailable treatment because this build binds
//!   no media index, no album map and no place map - see `filters_available`
//! - Every drawn affordance and its hit region come from the same `Rect`, so a
//!   tap is accepted exactly where something is painted
//! - Every string is measured with `interos_std::design::font::measure` /
//!   `ellipsize` at the coordinates it is drawn at, so text never clips or overlaps
//! - Banded rendering (`render_gallery_band`): only the rows an input actually
//!   dirtied are re-rasterised, instead of all 1,036,800 pixels on every event
//! - Asynchronous event reactor loop, graceful lifecycle supervision and clean
//!   return to the Launcher

#![no_std]
#![no_main]

mod outline_text;

use core::panic::PanicInfo;
use interos_std as std;
use std::design::font;
use std::design::primitives::{draw_pill, draw_rounded_rect, Rect};
use std::design::radius;
use std::design::spacing;
use std::design::touch;

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

/// The system home indicator: a 200x6 pill drawn inside the 240x40 gesture
/// strip that the tap handler accepts.
///
/// WHY two rectangles and not one: the drawn indicator is 6px of ink, which is
/// not a touch target, so the hit box has to be the whole strip - but the strip
/// itself has to be a rectangle that is actually painted, otherwise the tap
/// region drifts. The old code tested `x 240..480 && y >= 1400` with no upper
/// bound on y, a 241x40px region that nothing ever drew, so a tap 18 rows above
/// the indicator exited the app. Both rectangles live here, so the strip that is
/// hit-tested is the strip that contains the ink.
const HOME_HIT: Rect = Rect::new(240, 1400, 240, 40);
const HOME_PILL: Rect = Rect::new(260, 1418, 200, 6);

/// Rows the filter-chip band owns.
///
/// WHY exactly the drawn pill band: `pill_pixel` returns `None` outside
/// `PILL_Y..PILL_Y + PILL_H`, and every run of text inside it is gated on its
/// own measured line box, which is inside the pill. No other row can hold a
/// pixel of this control, so widening the band would re-rasterise rows that
/// cannot have changed.
const FILTER_BAND_Y0: usize = PILL_Y;
const FILTER_BAND_Y1: usize = PILL_Y + PILL_H;

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
    // The outline face, chosen ONCE for the run rather than once per pixel.
    //
    // WHY the order matters here specifically: this funnel is called for every
    // pixel of every label, and a per-pixel choice would draw the glyph from
    // both faces at once -- the outline's edge with a hard bitmap ghost one
    // pixel off, on every string on the surface. The launcher's first version of
    // this change did exactly that and it was only visible in the capture.
    if let Some(run) =
        outline_text::OutlineRun::place_run(x0, y0 + font::BASELINE * scale, scale, s, false)
    {
        // WHY the window is the run's and not the cell's: at the em that matches
        // this scale the outline's ascent reaches 18px above the cell origin and
        // its descent 4px past the bottom, so the cell-shaped test above would
        // clip tall glyphs off the top without anything looking wrong. The run's
        // own bounds admit exactly its ink and nothing more, so widening to them
        // cannot put a pixel where a glyph is not.
        let (rx, rtop, rw, rh) = run.bounds();
        if x >= rx && x - rx < rw && y >= rtop && y - rtop < rh {
            return run.pixel(x, y, color, bg);
        }
        // Inside the cell window but outside the run's ink: the caller's
        // background shows through, exactly as an uninked bitmap pixel would.
        return None;
    }
    // The bitmap face is the fallback, not the baseline: a refusal here -- a
    // bold run, a glyph whose ink leaves the window, a run wider than the
    // buffer, a baseline too near the top of the surface -- must leave the text
    // drawn rather than dropped.
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

/// Whether the segmented filter chips can select anything at all.
///
/// WHY derived from the ledger and not a hand-set flag: `PHOTOS` is this app's
/// only media source, and it is filled from real storage. An empty ledger means
/// there is also no album map and no favourites map behind the row, so `Albums`
/// and `Favorites` could only ever select the same empty set as `All`. The old
/// build drew all three chips in the live style and let a tap repaint one of
/// them, which is a control that looks live and changes nothing a user can see
/// below it. When this returns false the chips are drawn recessed and are not
/// hit-tested; when it returns true they are live segmented filters again.
#[inline]
fn filters_available() -> bool {
    !PHOTOS.is_empty()
}

/// How many bottom-bar destinations this build can actually reach.
///
/// WHY 1 and not 3: nothing bound to this app returns an album list or a place
/// list - the VFS exposes no such index and the platform binds no service that
/// does - so `Albums` and `Places` had no screen behind them. The old bar let a
/// tap change a label's ink and called that navigation. Entries at or past this
/// index are drawn recessed and are not hit-tested.
const REAL_SECTIONS: usize = 1;

/// Ink for an affordance that is painted but has no capability behind it.
///
/// WHY the border tone and not a grey invented here: it is the design system's
/// own hairline, so an unavailable control reads as recessed chrome against both
/// the page fill and a card, and it stays in step with the rest of the shell if
/// the hairline token ever moves.
const DISABLED: u32 = BORDER;

pub struct GalleryApp {
    pub selected_tab: usize, // 0: All, 1: Albums, 2: Favorites
    pub inspecting_photo: Option<usize>,
}

impl GalleryApp {
    pub const fn new() -> Self {
        Self {
            selected_tab: 0,
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
    let available = filters_available();
    for (i, label) in PILL_LABELS.iter().enumerate() {
        let rect = pill_rect(i);
        if !rect.contains(x, y) {
            continue;
        }
        // WHY the three-way treatment: an available row keeps the live
        // segmented-control grammar (filled = selected, soft = at rest), and an
        // unavailable row drops onto the page fill with a hairline and muted ink,
        // which is how the rest of the shell paints something it cannot do.
        // It is not hit-tested in that state, so the two agree.
        let (fill, ink, border) = if !available {
            (BG, MUTED, Some(BORDER))
        } else if app.selected_tab == i {
            (TEXT, INVERSE, None)
        } else {
            (SOFT, TEXT, Some(BORDER))
        };
        let shape = draw_pill(x, y, rect, fill, border)?;
        if shape != fill {
            return Some(shape);
        }
        let lw = font::measure(label, 2);
        let lx = rect.x + rect.w.saturating_sub(lw) / 2;
        if let Some(c) = text(x, y, lx, PILL_Y, 2, label, ink, fill) {
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
        // WHY these two extra lines: they are the on-screen reason the filter
        // chips above and the two extra section-bar entries below are painted
        // recessed. Without them a user sees three chips and three tabs that do
        // nothing and has no way to find out why. Each line is one measured
        // caption on the row below the previous one (24px cells at 8px pitch),
        // well clear of the pictogram above and the grid floor below.
        let filters_note = b"Filters need a media index; none is bound.";
        if let Some(c) = text_centered(x, y, MARGIN, 786, W - 2 * MARGIN, 1, filters_note, MUTED, BG)
        {
            return Some(c);
        }
        let sections_note = b"Albums and Places need their own index.";
        if let Some(c) = text_centered(x, y, MARGIN, 818, W - 2 * MARGIN, 1, sections_note, MUTED, BG)
        {
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
// Bottom section bar
// -----------------------------------------------------------------------------
/// WHY the bar takes no `&GalleryApp`: with `REAL_SECTIONS == 1` there is
/// exactly one reachable destination, so a stored "selected tab" would always be
/// 0 and would only pretend to be state. The bar marks where this screen is, and
/// marks the destinations it cannot reach as recessed.
fn nav_pixel(x: usize, y: usize) -> Option<u32> {
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
        // WHY the hairline ink past the real destinations: it is the same token
        // the unavailable filter chips use, so "painted but not actionable" looks
        // identical wherever the user meets it.
        let color = if i < REAL_SECTIONS { TEXT } else { DISABLED };
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
/// The inspection app bar's back control.
///
/// WHY a named constant: the old code wrote `Rect::new(MARGIN, 28, 56, 56)` in
/// the renderer and again, literally, in the tap handler. Two copies of a
/// geometry is two chances for the drawn pill and the hit box to disagree, and
/// 56 is the shell's primary action target, so it is taken from the scale rather
/// than retyped.
const DETAIL_BACK: Rect = Rect::new(MARGIN, 28, touch::PRIMARY_ACTION, touch::PRIMARY_ACTION);

fn detail_pixel(x: usize, y: usize, _app: &GalleryApp, idx: usize) -> u32 {
    let Some(photo) = PHOTOS.get(idx) else {
        return BG;
    };

    // App bar: back control + record title/meta, each measured to fit.
    if y < DETAIL_BAR_H {
        if y == DETAIL_BAR_H - 1 {
            return BORDER;
        }
        if let Some(c) = draw_pill(x, y, DETAIL_BACK, SOFT, Some(BORDER)) {
            if c != SOFT {
                return c;
            }
            // WHY the centre is derived, not typed: the chevron apex sits at the
            // pill's left third and its vertical middle, both taken from
            // `DETAIL_BACK`, so moving the pill cannot orphan the glyph.
            let cx = DETAIL_BACK.x + DETAIL_BACK.w / 4;
            let cy = DETAIL_BACK.y + DETAIL_BACK.h / 2;
            if chevron_left(x, y, cx, cy, 12) {
                return TEXT;
            }
            return SOFT;
        }

        let title = photo.title.as_bytes();
        // WHY `DETAIL_BACK.x + DETAIL_BACK.w + spacing::GRID_16` rather than the
        // literal 96: the run starts a fixed gap past the right edge of the
        // control it sits beside, so the two cannot collide if the control moves.
        let text_x = DETAIL_BACK.x + DETAIL_BACK.w + spacing::GRID_16;
        let text_w = W - text_x - MARGIN;
        if y >= 28 && y < 28 + font::line_height(2) {
            let mut buf = [0u8; 48];
            let n = font::ellipsize(title, 2, text_w, &mut buf);
            if let Some(c) = text(x, y, text_x, 28, 2, &buf[..n], TEXT, SURFACE) {
                return c;
            }
        }
        let meta = photo.meta.as_bytes();
        if y >= 80 && y < 80 + font::line_height(1) {
            let mut buf = [0u8; 64];
            let n = font::ellipsize(meta, 1, text_w, &mut buf);
            if let Some(c) = text(x, y, text_x, 80, 1, &buf[..n], MUTED, SURFACE) {
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
            radius::SMALL,
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
        let lx = close.x + (close.w.saturating_sub(lw)) / 2;
        // WHY the measured centring, kept: a 48px glyph cell inside a 56px pill
        // is 4px of slack top and bottom; anything less and the descenders of
        // the face clip against the pill edge.
        let ly = close.y + (close.h.saturating_sub(font::line_height(2))) / 2;
        if let Some(c) = text(x, y, lx, ly, 2, label, TEXT, SOFT) {
            return c;
        }
        return SOFT;
    }

    // Home gesture indicator.
    if HOME_PILL.contains(x, y) {
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

    // 2. Segmented filter chips.
    if let Some(c) = pill_pixel(x, y, app) {
        return c;
    }

    // 3. Thumbnail grid band (or honest empty state).
    if let Some(c) = grid_pixel(x, y, app) {
        return c;
    }

    // 4. Bottom section bar.
    if let Some(c) = nav_pixel(x, y) {
        return c;
    }

    // 5. Home gesture indicator.
    if HOME_PILL.contains(x, y) {
        return MUTED;
    }

    BG
}

fn render_gallery_ui(surface: u32, app: &GalleryApp) {
    render_gallery_band(surface, app, 0, H);
}

/// Re-rasterises and writes only rows `y0..y1`.
///
/// WHY a band and not a rectangle: a row of the surface is one contiguous run in
/// the surface buffer, so `offset = y0 * W` writes a whole band with no stride
/// bookkeeping. Every band starts on a row boundary, so the running `x`/`y`
/// cursor below is exact.
///
/// WHY this is correct rather than a shortcut: the band is derived from what the
/// input actually changed, and the pixels outside it are correct because nothing
/// about them changed. The old renderer re-rasterised all 1,036,800 pixels on
/// every event, so a tap that changed nothing cost a full-screen pass.
fn render_gallery_band(surface: u32, app: &GalleryApp, y0: usize, y1: usize) {
    let mut chunk = [0u32; CHUNK];
    let mut offset = y0 * W;
    let end = y1.min(H) * W;
    while offset < end {
        let count = (end - offset).min(CHUNK);
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
// Input handling. Every region below is hit-tested against the same `Rect` the
// renderer drew, so a control is accepted exactly where it is painted.
// -----------------------------------------------------------------------------

/// Which rows of the surface an input actually changed.
///
/// WHY this exists: the old loop called `render_gallery_ui` from every branch
/// that touched the model, which re-rasterised all 1,036,800 pixels - so a tap
/// on an empty grid, or on one of the two destinations with nothing behind it,
/// cost a full-screen pass to redraw an identical screen.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    /// Nothing changed; do not render at all.
    None,
    /// A filter chip was selected: only the chip row moved.
    FilterRow,
    /// A photo was opened in the inspection view: the whole screen is replaced.
    All,
}

/// Grow a dirty region so one rendered frame covers everything a poll batch
/// changed.
///
/// WHY: `event_poll` can deliver several taps before the app is scheduled again,
/// and each of them reports its own band. Rendering only the last one would
/// leave the earlier tap's pixels stale.
impl Dirty {
    fn widen(self, other: Dirty) -> Dirty {
        match (self, other) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            // Every non-`None` region is either the chip row or the whole screen,
            // so anything combined with the screen stays the screen.
            _ => Dirty::All,
        }
    }
}

enum Tap {
    /// Nothing on screen moved; the surface is already correct.
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

fn handle_tap(app: &mut GalleryApp, x: usize, y: usize) -> Tap {
    // Bottom home gesture: closes the inspection view, else leaves the app.
    // WHY `HOME_HIT` and not the raw numbers: the strip that is hit-tested is the
    // same strip the indicator is drawn in, so the tap region can no longer be a
    // region nothing was ever painted on.
    if HOME_HIT.contains(x, y) {
        if app.inspecting_photo.is_some() {
            app.inspecting_photo = None;
            std::log(0, "[GALLERY] Exited photo inspection view.").ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::Leave("[GALLERY] Home gesture pressed. Exiting to launcher.");
    }

    // Inspection view controls.
    if app.inspecting_photo.is_some() {
        let close = Rect::new(CLOSE_X0, CLOSE_Y0, CLOSE_W, CLOSE_H);
        if DETAIL_BACK.contains(x, y) || close.contains(x, y) {
            app.inspecting_photo = None;
            std::log(0, "[GALLERY] Closed photo inspection view.").ok();
            return Tap::Redraw(Dirty::All);
        }
        return Tap::None;
    }

    // Segmented filter chips - hit-tested only when they are drawn live.
    //
    // WHY the availability gate: `pill_pixel` draws the row recessed when
    // `filters_available()` is false, and a recessed control that still accepted
    // a tap would be exactly the dead affordance this replaces. The two states
    // are decided by the same function, so they cannot disagree.
    if filters_available() && y >= FILTER_BAND_Y0 && y < FILTER_BAND_Y1 {
        for i in 0..PILL_LABELS.len() {
            if pill_rect(i).contains(x, y) {
                app.selected_tab = i;
                std::log(0, "[GALLERY] Gallery filter chip switched.").ok();
                return Tap::Redraw(Dirty::FilterRow);
            }
        }
        return Tap::None;
    }

    // Thumbnail grid taps.
    if y >= GRID_TOP && y < GRID_BOTTOM {
        if has_no_media() {
            std::log(
                0,
                "[GALLERY] Photo grid tapped: no media items in storage.",
            )
            .ok();
            return Tap::None;
        }
        if let (Some(col), Some(row)) = (col_of(x), row_of(y)) {
            let p_idx = row * 3 + col;
            if PHOTOS.get(p_idx).is_some() {
                app.inspecting_photo = Some(p_idx);
                std::log(0, "[GALLERY] Photo tapped: inspection view opened.").ok();
                return Tap::Redraw(Dirty::All);
            }
        }
        return Tap::None;
    }

    // The section bar marks where this screen is. `Albums` and `Places` have no
    // index behind them, so they are not controls and are not hit-tested - the
    // old code took the tap, stored a new index and repainted a label colour,
    // which is the same "looks live, goes nowhere" affordance.
    Tap::None
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
                let mut dirty = Dirty::None;
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
                        exit_to_launcher(surface);
                    }

                    if input.kind == 1 {
                        match handle_tap(&mut app, input.x as usize, input.y as usize) {
                            Tap::Leave(msg) => {
                                std::log(0, msg).ok();
                                exit_to_launcher(surface);
                            }
                            Tap::Redraw(d) => dirty = dirty.widen(d),
                            Tap::None => {}
                        }
                    }
                }
                if dirty != Dirty::None && surface > 0 {
                    match dirty {
                        // The chip row is the only control on this screen whose
                        // state is local, so it is the only band a chip tap can
                        // dirty. Everything else that changes state changes the
                        // whole screen and asks for `All`.
                        Dirty::FilterRow => {
                            render_gallery_band(surface, &app, FILTER_BAND_Y0, FILTER_BAND_Y1)
                        }
                        Dirty::All => render_gallery_ui(surface, &app),
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
    std::log(0, "[GALLERY] Panic detected! Terminating task...").ok();
    std::exit();
}
