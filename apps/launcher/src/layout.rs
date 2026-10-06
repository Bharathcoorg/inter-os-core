//! Geometry that is computed, not typed in.
//!
//! WHY this module exists: the dock previously assigned six fixed positions and
//! six hardcoded labels — "Phone, Messages, Camera / Files, Browser, Apps" —
//! regardless of what was installed. Uninstall the camera app and its tile
//! remained, now a dead button. The log line beside it said "10-app home screen"
//! and "16-app grid" as literal strings, so both the layout and the description of
//! the layout were fiction that happened to be stable.
//!
//! Everything here derives from a count. A grid with 3 items and a grid with 30
//! items run the same code. Screens pass in what exists; this module decides where
//! it goes. That is what makes the shell adapt to available apps instead of
//! describing them.

use crate::theme::{space, Theme, MIN_TOUCH_TARGET, SCREEN_HEIGHT, SCREEN_WIDTH};

/// A rectangle in logical surface coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    pub const fn new(x: usize, y: usize, w: usize, h: usize) -> Rect {
        Rect { x, y, w, h }
    }

    /// Horizontal centre.
    pub fn center_x(&self) -> usize {
        self.x + self.w / 2
    }

    /// Vertical centre.
    pub fn center_y(&self) -> usize {
        self.y + self.h / 2
    }

    /// Whether a logical point falls inside, inclusive of the far edges.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// Shrinks by `inset` on every side, never past zero.
    pub fn inset(&self, inset: usize) -> Rect {
        Rect {
            x: self.x + inset,
            y: self.y + inset,
            w: self.w.saturating_sub(inset * 2),
            h: self.h.saturating_sub(inset * 2),
        }
    }

    /// Squared distance from a point to this rect's centre.
    pub fn center_distance_sq(&self, x: usize, y: usize) -> i64 {
        let dx = x as i64 - self.center_x() as i64;
        let dy = y as i64 - self.center_y() as i64;
        dx * dx + dy * dy
    }
}

/// A uniform grid sized to fit `count` items inside `bounds`.
///
/// WHY `max_rows` exists: a grid of 30 apps must not become 30 rows. The caller
/// decides how tall the screen may grow and the grid scrolls within that, so a
/// large catalogue cannot push content off the surface.
pub struct Grid {
    pub bounds: Rect,
    pub columns: usize,
    pub rows: usize,
    pub cell: Rect,
    pub gutter: usize,
    pub origin_x: usize,
    pub origin_y: usize,
}

/// Computes a grid for `count` items.
///
/// Columns are chosen first, from the available width, and rows follow. Items are
/// centred within the grid so a partially filled last row does not leave the row
/// hugging the left edge while the rest of the screen is symmetric.
pub fn grid(count: usize, bounds: Rect, max_rows: usize, gutter: usize) -> Grid {
    let columns = grid_columns(count, bounds.w, gutter);
    let rows = ((count + columns - 1) / columns).max(1).min(max_rows.max(1));
    let cell_w = (bounds.w - gutter * (columns - 1)) / columns.max(1);
    let cell_h = (bounds.h - gutter * (rows - 1)) / rows.max(1);
    // Centre a short final row so the grid stays visually balanced.
    let used_in_row = count.min(columns);
    let slack = columns - used_in_row;
    let origin_x = bounds.x + if slack > 0 { (gutter + cell_w) * slack / 2 } else { 0 };

    Grid {
        bounds,
        columns,
        rows,
        cell: Rect::new(0, 0, cell_w, cell_h),
        gutter,
        origin_x,
        origin_y: bounds.y,
    }
}

/// Computes a grid with an explicit number of columns.
pub fn grid_fixed_columns(count: usize, bounds: Rect, columns: usize, max_rows: usize, gutter: usize) -> Grid {
    let columns = columns.max(1);
    let rows = ((count + columns - 1) / columns).max(1).min(max_rows.max(1));
    let cell_w = (bounds.w.saturating_sub(gutter * (columns - 1))) / columns;
    let cell_h = (bounds.h.saturating_sub(gutter * (rows - 1))) / rows;
    Grid {
        bounds,
        columns,
        rows,
        cell: Rect::new(0, 0, cell_w, cell_h),
        gutter,
        origin_x: bounds.x,
        origin_y: bounds.y,
    }
}

/// How many columns fit a width, clamped to a sane range.
///
/// WHY a floor of one and a ceiling of six: zero columns would divide by zero, and
/// more than six columns on a 720px surface puts each tile under 96px, which is
/// below the touch target the rest of the system guarantees.
pub fn grid_columns(count: usize, width: usize, gutter: usize) -> usize {
    if count == 0 {
        return 1;
    }
    let mut cols = 1usize;
    while cols < count && cols < 6 {
        let need = MIN_TOUCH_TARGET + gutter;
        if (cols + 1) * need <= width {
            cols += 1;
        } else {
            break;
        }
    }
    cols.max(1)
}

/// The rectangle for the `index`-th cell of a grid, or `None` if out of range.
pub fn grid_cell(g: &Grid, index: usize) -> Option<Rect> {
    if index >= g.columns * g.rows {
        return None;
    }
    let col = index % g.columns;
    let row = index / g.columns;
    Some(Rect::new(
        g.origin_x + col * (g.cell.w + g.gutter),
        g.origin_y + row * (g.cell.h + g.gutter),
        g.cell.w,
        g.cell.h,
    ))
}

/// Which grid cell contains a point, if any.
pub fn grid_hit(g: &Grid, x: usize, y: usize) -> Option<usize> {
    for i in 0..g.columns * g.rows {
        if let Some(cell) = grid_cell(g, i) {
            if cell.contains(x, y) {
                return Some(i);
            }
        }
    }
    None
}

/// The bottom dock: full width, pinned above the home indicator.
pub fn dock(bounds: Rect, height: usize) -> Rect {
    Rect::new(bounds.x, bounds.y + bounds.h - height, bounds.w, height)
}

/// Vertical stack of `count` rows filling `bounds`, each `row_height` tall.
pub fn list(bounds: Rect, count: usize, row_height: usize, gutter: usize) -> Option<Rect> {
    if count == 0 {
        return None;
    }
    let h = count * row_height + (count - 1) * gutter;
    Some(Rect::new(bounds.x, bounds.y, bounds.w, h.min(bounds.h)))
}

/// The rectangle for the `index`-th row of a stack produced by [`list`].
pub fn list_row(l: Rect, index: usize, count: usize, row_height: usize, gutter: usize) -> Option<Rect> {
    if index >= count {
        return None;
    }
    Some(Rect::new(l.x, l.y + index * (row_height + gutter), l.w, row_height))
}

/// Splits `count` items into `per_page` pages.
pub fn pages(count: usize, per_page: usize) -> usize {
    if per_page == 0 {
        return 0;
    }
    count.div_ceil(per_page).max(1)
}

/// The slice of item indices shown on `page`.
pub fn page_slice(count: usize, per_page: usize, page: usize) -> (usize, usize) {
    if per_page == 0 {
        return (0, 0);
    }
    let start = (page * per_page).min(count);
    let end = (start + per_page).min(count);
    (start, end)
}

/// Full-screen content area below the status bar and above the nav bar.
pub fn content_area(theme_pad: usize) -> Rect {
    let top = crate::theme::STATUS_BAR_HEIGHT + theme_pad;
    Rect::new(
        theme_pad,
        top,
        SCREEN_WIDTH - theme_pad * 2,
        SCREEN_HEIGHT - top - crate::theme::NAV_BAR_HEIGHT - theme_pad,
    )
}

/// A card centred inside `bounds`, inset by `space::MD` on every side.
pub fn card(bounds: Rect) -> Rect {
    bounds.inset(space::MD)
}

/// Picks foreground ink that stays legible on `bg`.
///
/// WHY this is computed rather than passed: half the surfaces in this shell are
/// theme-derived, so a hardcoded ink colour is wrong on exactly one of the two
/// themes. Relative luminance with the usual sRGB gamma expansion is the standard
/// test and matches human perception closely enough for UI chrome.
///
/// The threshold is the perceptual crossover rather than the arithmetic 0.5:
/// mid-grey at luminance 0.179 already reads as "light background" to the eye,
/// which is why surfaces in the 0.2 range need dark ink even though half their
/// channels are above 0.5.
/// The perceptual crossover. Mid-grey at this luminance already reads as a "light"
/// background to the eye, which is why surfaces in the 0.2 range need dark ink
/// even though half their channels are above the arithmetic midpoint.
const LUMINANCE_CROSSOVER: u32 = 46; // ~0.179 * 255

pub fn ink_on(theme: &Theme, bg: u32) -> u32 {
    let lin = |c: u8| -> u32 {
        let v = c as u32;
        // Integer gamma-2 approximation of the sRGB decode. `powf` is unavailable
        // in no_std without libm, and squaring the normalised channel is close
        // enough to decide ink polarity, which is all this is for.
        (v * v) >> 8
    };
    // Channel positions in 0xAARRGGBB: alpha 24, red 16, green 8, blue 0.
    // Reading red from shift 24 samples the alpha channel, which is 0xFF on every
    // opaque colour, so every background measured as equally light and ink polarity
    // never flipped.
    let lum = (2126 * lin(((bg >> 16) & 0xFF) as u8)
        + 7152 * lin(((bg >> 8) & 0xFF) as u8)
        + 722 * lin((bg & 0xFF) as u8))
        / 10000;
    if lum > LUMINANCE_CROSSOVER {
        theme.ink_on_light
    } else {
        theme.ink_on_dark
    }
}