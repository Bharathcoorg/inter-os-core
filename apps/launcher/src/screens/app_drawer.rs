//! App Drawer: the catalogue grid, derived from what is actually installed.
//!
//! WHY `pub`: `render_view` in the crate root dispatches here by view mode, and
//! the shell's touch handler calls [`app_drawer_hit`] so that a tap is resolved
//! with the same geometry the screen draws with instead of a second copy of it.
//!
//! WHY paging rather than scrolling: the surface is a fixed 720x1440 and the
//! catalogue is a `[[u8; 16]; 32]`, so it can hold more apps than one screen can
//! show. Paging rides on `state.app_drawer_page`, which the shell already moves
//! on a horizontal swipe; scrolling would need an offset `LauncherState` does not
//! carry, and motion a per-pixel renderer has nowhere to put. Paging also keeps
//! every app one deliberate swipe away and says where you are with a dot row.
//!
//! WHY the rest is computed: the previous version drew twelve fixed 180x210 cells
//! at hand-typed coordinates, so the bottom three rows were empty card outlines
//! whenever fewer than nine apps were installed, and the log beside it claimed a
//! sixteen-app grid whatever the registry held. Nothing in this file knows how
//! many apps exist.

use crate::{test_text, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::{
    radius, space, type_scale, Theme, CONTENT_PADDING, MIN_TOUCH_TARGET, NAV_BAR_HEIGHT,
    SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT,
};

/// Rows the drawer shows at once.
///
/// WHY four: the band between the filter strip and the dot row is 1,072px tall, and
/// a tile holds an icon, a gap and a line of name. Four rows keep the tile close to
/// square (196x256 at three columns) instead of letting a two-row catalogue stretch
/// each tile to 544px. It is a viewport capacity, not a description of the
/// catalogue: what a page holds is `columns * DRAWER_ROWS`.
const DRAWER_ROWS: usize = 4;

/// Gap between tiles, off the theme's spacing scale.
const TILE_GUTTER: usize = space::MD;

/// Longest name the catalogue can carry: the element width of the label buffer in
/// `LauncherState::app_labels`. Read from the state layout rather than assumed.
const APP_LABEL_CAPACITY: usize = 16;

/// Type scale for an app's name, and for the badge glyph inside its icon.
const LABEL_SCALE: usize = type_scale::CAPTION;
const BADGE_SCALE: usize = type_scale::BADGE;

/// Icon badge: at most three glyphs, which is what the icon field in
/// `LauncherState::app_icons` is there to hold.
const BADGE_GLYPHS: usize = 3;

/// Diameter of the pagination dot, and the gap between dots.
const DOT_D: usize = space::SM;
const DOT_GAP: usize = space::MD;

/// Height of the home gesture pill.
///
/// WHY 6 and not a spacing token: the pill is an indicator, not a control, and on a
/// 1,440px surface a 4px line reads as a rendering fault while an 8px bar reads as a
/// button. Six is the line.
const GESTURE_PILL_H: usize = 6;

/// Capacity of the formatted render-complete log line.
// WHY allowed dead: the entry points below are what the shell calls once it wires
// the drawer hit test and the render log (see the module report). Until then they
// are reachable only through `screens::app_drawer`, and rustc's reachability pass
// for a binary crate does not see that, so every one of them warns. The alternative
// was deleting the shell's half of the contract out of the screen that owns it.
#[allow(dead_code)]
pub const APP_DRAWER_LOG_CAP: usize = 176;

/// Verbatim tail every render-complete line must keep: the lifecycle harness waits
/// for this exact substring.
const LOG_TAIL: &[u8] = b"App drawer surface rendered.";

// -----------------------------------------------------------------------------
// Geometry
// -----------------------------------------------------------------------------

/// The drawer's chrome, stacked down the screen from the theme's metrics.
struct Frame {
    search: Rect,
    chips: Rect,
    grid: Rect,
    dots: Rect,
    pill: Rect,
}

fn frame() -> Frame {
    let content = layout::content_area(CONTENT_PADDING);
    let search = Rect::new(content.x, content.y, content.w, MIN_TOUCH_TARGET);
    let chips_y = search.y + search.h + space::SM;
    let chips = Rect::new(content.x, chips_y, content.w, MIN_TOUCH_TARGET);
    let dots = Rect::new(
        content.x,
        content.y + content.h - space::XL,
        content.w,
        space::XL,
    );
    let grid_top = chips.y + chips.h + space::MD;
    let grid = Rect::new(
        content.x,
        grid_top,
        content.w,
        dots.y - space::LG - grid_top,
    );
    let nav_top = SCREEN_HEIGHT - NAV_BAR_HEIGHT;
    let pill = Rect::new(
        (SCREEN_WIDTH - space::XXL * 4) / 2,
        nav_top + (NAV_BAR_HEIGHT - GESTURE_PILL_H) / 2,
        space::XXL * 4,
        GESTURE_PILL_H,
    );
    Frame {
        search,
        chips,
        grid,
        dots,
        pill,
    }
}

/// The label width that half of the catalogue's names fit inside.
///
/// WHY the median and not the longest name: a tile sized for the single longest
/// name in the registry ("Camera Preview", fourteen glyphs) drops the grid to two
/// columns and leaves every other app marooned in a 302px cell. Sizing the tile so
/// half the catalogue reads in full holds the grid at the columns the design uses
/// and leaves the few genuinely long names to `text::ellipsize`, which is what an
/// ellipsis is for.
fn median_label_px(state: &LauncherState) -> usize {
    let mut hist = [0usize; APP_LABEL_CAPACITY + 1];
    for i in 0..state.app_count {
        hist[(state.app_label_lens[i] as usize).min(APP_LABEL_CAPACITY)] += 1;
    }
    let half = (state.app_count + 1) / 2;
    let mut seen = 0usize;
    for (len, hits) in hist.iter().enumerate() {
        seen += hits;
        if seen >= half {
            return len * text::GLYPH_W * LABEL_SCALE;
        }
    }
    0
}

/// Columns the drawer lays out.
///
/// WHY two ceilings and not one: `layout::grid_columns` answers "how many cells fit
/// this width while each still meets the minimum touch target", which on a 720px
/// surface is six columns of about 106px. A drawer tile is not a bare target -- it
/// is an icon with the app's name under it -- and at the theme's caption scale a
/// 106px cell holds five glyphs, which turns "Calculator" into "Calc...". So the
/// grid takes whichever ceiling is tighter: the surface's touch ceiling, or the
/// width the catalogue's own labels need plus `space::MD` of air on each side.
pub fn drawer_columns(state: &LauncherState) -> usize {
    let content = layout::content_area(CONTENT_PADDING);
    let tile_min = median_label_px(state) + space::MD * 2;
    let by_touch = layout::grid_columns(state.app_count, content.w, TILE_GUTTER);
    let by_label = (content.w + TILE_GUTTER) / (tile_min + TILE_GUTTER);
    by_touch.min(by_label).max(1)
}

/// How many apps one page of the drawer holds for the current catalogue.
pub fn app_drawer_per_page(state: &LauncherState) -> usize {
    drawer_columns(state) * DRAWER_ROWS
}

/// How many pages the current catalogue needs.
pub fn app_drawer_page_count(state: &LauncherState) -> usize {
    layout::pages(state.app_count, app_drawer_per_page(state))
}

/// The page actually shown, clamped into range.
///
/// WHY clamp rather than trust: `app_drawer_page` is a bare `usize` the shell
/// mutates, and a catalogue that shrank underneath it would otherwise render an
/// empty page and claim there were two of them.
pub fn app_drawer_page(state: &LauncherState) -> usize {
    state.app_drawer_page.min(app_drawer_page_count(state) - 1)
}

/// The cell rectangle for slot `slot` of the current page, at the drawer's stable
/// cell size.
///
/// WHY sized from the page capacity and not from the page's item count: cell size
/// has to be the same on every page or the grid reflows as you swipe.
fn current_grid(capacity: usize) -> layout::Grid {
    layout::grid(capacity, frame().grid, DRAWER_ROWS, TILE_GUTTER)
}

/// Which catalogue index a touch at (`x`, `y`) lands on, if any.
///
/// WHY this lives here and not in the shell: the shell's own hit test re-derived
/// the grid from hardcoded row and column bands, so any change to the layout would
/// have left taps pointing at the wrong app. This is the same geometry the screen
/// draws with, so the two cannot drift.
/// app_drawer_hit, app_drawer_dot_at, app_drawer_log and log_render are the shell's
/// half of this screen's contract and are documented as such; they carry the same
/// allowance as [`APP_DRAWER_LOG_CAP`] above.
#[allow(dead_code)]
pub fn app_drawer_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    if state.app_count == 0 {
        return None;
    }
    let capacity = app_drawer_per_page(state);
    let g = current_grid(capacity);
    let slot = slot_at(&g, x, y)?;
    let (start, end) = layout::page_slice(state.app_count, capacity, app_drawer_page(state));
    if slot >= end - start {
        return None;
    }
    Some(start + slot)
}

/// Which page the pagination dot at (`x`, `y`) jumps to, if any.
#[allow(dead_code)]
pub fn app_drawer_dot_at(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    let total = app_drawer_page_count(state);
    if total <= 1 {
        return None;
    }
    let f = frame();
    if !f.dots.contains(x, y) {
        return None;
    }
    let pitch = DOT_D + DOT_GAP;
    let span = total * pitch - DOT_GAP;
    let x0 = f.dots.x + (f.dots.w - span) / 2;
    let cy = f.dots.center_y();
    for page in 0..total {
        let cx = x0 + page * pitch + DOT_D / 2;
        let dx = x as i64 - cx as i64;
        let dy = y as i64 - cy as i64;
        if dx * dx + dy * dy <= (DOT_D as i64) * (DOT_D as i64) {
            return Some(page);
        }
    }
    None
}

/// Grid slot under a point, computed from the grid's own pitch rather than by
/// testing cells one at a time.
fn slot_at(g: &layout::Grid, x: usize, y: usize) -> Option<usize> {
    let pitch_x = g.cell.w + g.gutter;
    let pitch_y = g.cell.h + g.gutter;
    if pitch_x == 0 || pitch_y == 0 || x < g.origin_x || y < g.origin_y {
        return None;
    }
    let col = (x - g.origin_x) / pitch_x;
    let row = (y - g.origin_y) / pitch_y;
    if col >= g.columns || row >= g.rows {
        return None;
    }
    Some(row * g.columns + col)
}

// -----------------------------------------------------------------------------
// Catalogue metadata
// -----------------------------------------------------------------------------

fn app_label(state: &LauncherState, index: usize) -> &[u8] {
    let len = (state.app_label_lens[index] as usize).min(APP_LABEL_CAPACITY);
    &state.app_labels[index][..len]
}

fn app_badge(state: &LauncherState, index: usize) -> &[u8] {
    &state.app_icons[index][..BADGE_GLYPHS]
}

/// The rotation an app's icon is drawn in.
///
/// WHY four and not six: the rotation reads from theme fields only, and the theme
/// has exactly four chromatic tokens. With three columns no two adjacent tiles can
/// share a rotation, so the grid still reads as distinct apps.
fn hue_for(theme: &Theme, index: usize) -> u32 {
    const ROTATION_KEY: usize = 4;
    match index % ROTATION_KEY {
        0 => theme.accent,
        1 => theme.success,
        2 => theme.warning,
        _ => theme.danger,
    }
}

/// Copies bytes into `buf` at `at`, stopping at the end of the buffer.
fn put(buf: &mut [u8], at: &mut usize, bytes: &[u8]) {
    for b in bytes {
        if *at >= buf.len() {
            return;
        }
        buf[*at] = *b;
        *at += 1;
    }
}

/// Formats an integer into `buf` without an allocator.
fn push_usize(buf: &mut [u8], at: &mut usize, value: usize) {
    if value == 0 {
        put(buf, at, b"0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    let mut v = value;
    while v > 0 && n < digits.len() {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        put(buf, at, &digits[i..i + 1]);
    }
}

/// `prefix` + count + `suffix`, into a caller-owned buffer.
fn join_count(prefix: &[u8], count: usize, suffix: &[u8], buf: &mut [u8]) -> usize {
    let mut at = 0usize;
    put(buf, &mut at, prefix);
    push_usize(buf, &mut at, count);
    put(buf, &mut at, suffix);
    at
}

/// The search field's placeholder, counted rather than asserted.
fn search_placeholder(state: &LauncherState) -> ([u8; 32], usize) {
    let mut buf = [0u8; 32];
    let n = join_count(b"Search ", state.app_count, b" apps", &mut buf);
    (buf, n)
}

/// The filter chip's label: the whole catalogue, counted.
fn chip_label(state: &LauncherState) -> ([u8; 32], usize) {
    let mut buf = [0u8; 32];
    let n = join_count(b"All ", state.app_count, b" apps", &mut buf);
    (buf, n)
}

// -----------------------------------------------------------------------------
// Render
// -----------------------------------------------------------------------------

pub fn get_app_drawer_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // WHY bound here rather than in a static: the shell has exactly one active
    // theme and every pixel on this screen resolves against it. A static would be a
    // second source of truth that no runtime setting could move. The shell should
    // hand the active theme in -- `LauncherState` grows a `theme: Theme` field fed
    // from the same setting that picks the wallpaper, and this line becomes
    // `state.theme`. Until then the binding is a value, not a constant, so the swap
    // is one edit in one place.
    let theme = Theme::LIGHT;
    let f = frame();

    if y < STATUS_BAR_HEIGHT {
        return status_bar(x, y, state, &theme);
    }
    if f.pill.contains(x, y) {
        return theme.text_muted;
    }
    if f.search.contains(x, y) {
        return search_field(x, y, state, &f.search, &theme);
    }
    if f.chips.contains(x, y) {
        return filter_chips(x, y, state, &f.chips, &theme);
    }
    if state.app_count == 0 {
        return empty_catalogue(x, y, &f.grid, &theme);
    }

    let capacity = app_drawer_per_page(state);
    let page_total = layout::pages(state.app_count, capacity);
    if page_total > 1 && f.dots.contains(x, y) {
        return pagination_dots(x, y, state, &f.dots, page_total, &theme);
    }

    let g = current_grid(capacity);
    let Some(slot) = slot_at(&g, x, y) else {
        return theme.bg;
    };
    let (start, end) = layout::page_slice(state.app_count, capacity, app_drawer_page(state));
    if slot >= end - start {
        // A short final page leaves real slots empty; they show the page, not a
        // blank card that pretends to be an app.
        return theme.bg;
    }
    app_tile(x, y, state, &g, slot, start, &theme)
}

/// Status bar: clock, signal, wifi, battery, on the theme's overlay chrome.
fn status_bar(x: usize, y: usize, state: &LauncherState, theme: &Theme) -> u32 {
    if y == STATUS_BAR_HEIGHT - 1 {
        return theme.border;
    }
    let mid = STATUS_BAR_HEIGHT / 2;

    // WHY the clock is gated on `clock_source_ok`: with no RTC authority granted the
    // read is refused, and drawing a stale or placeholder reading on chrome the user
    // glances at would present an unmeasured value as a measurement.
    if state.clock_source_ok {
        let clock_x = CONTENT_PADDING + space::SM;
        let clock_y = (STATUS_BAR_HEIGHT - text::line_height(LABEL_SCALE)) / 2;
        if let Some(c) = test_text(
            x,
            y,
            clock_x,
            clock_y,
            LABEL_SCALE,
            &state.clock_time,
            theme.text,
        ) {
            return c;
        }
    }

    // Battery, right-aligned on the content margin.
    let battery = Rect::new(
        SCREEN_WIDTH - CONTENT_PADDING - space::XXL - space::SM,
        mid - 7,
        space::XXL + space::SM,
        14,
    );
    let tip = Rect::new(battery.x + battery.w + 2, mid - 3, 4, 6);
    if inside_rounded(battery, x, y, radius::CHIP) {
        if !inside_rounded(battery.inset(1), x, y, radius::CHIP - 1) {
            return theme.text;
        }
        if battery.inset(space::SM - 4).contains(x, y) {
            return theme.success;
        }
        return theme.surface_sunken;
    }
    if tip.contains(x, y) {
        return theme.text;
    }

    // Wifi: two arcs and a dot, above and left of the battery.
    let wifi = Rect::new(
        battery.x - space::XL - space::SM,
        mid - 10,
        space::XXL,
        20,
    );
    let wcx = wifi.center_x() as i32;
    let wcy = wifi.y as i32 + 15;
    let dx = x as i32 - wcx;
    let dy = y as i32 - wcy;
    let d2 = dx * dx + dy * dy;
    if dy <= 0 && ((36..=64).contains(&d2) || (100..=144).contains(&d2)) {
        return theme.text;
    }
    if d2 <= 9 {
        return theme.text;
    }

    // Signal: four bars, shortest first, bottom-aligned to the chrome.
    let bars_right = wifi.x - space::MD;
    for i in 0..4usize {
        let h = 6 + i * 4;
        let bx = bars_right - (4 - i) * 4 - (3 - i) * 4;
        let bar = Rect::new(bx, mid + 8 - h, 4, h);
        if bar.contains(x, y) {
            return theme.text;
        }
    }

    theme.bg
}

/// The search field.
///
/// WHY the placeholder counts the catalogue: "Search apps..." is a fixed string on
/// a screen whose whole point is that the set of apps varies, and it claims a
/// capability the shell has not wired up -- typing into the drawer cannot filter it
/// today, because no text input path reaches this screen. The count is at least
/// true. Wiring the field is a shell change and is reported with the rest.
fn search_field(x: usize, y: usize, state: &LauncherState, field: &Rect, theme: &Theme) -> u32 {
    if !inside_rounded(*field, x, y, radius::CONTROL) {
        return theme.bg;
    }
    if !inside_rounded(field.inset(1), x, y, radius::CONTROL - 1) {
        return theme.border;
    }

    let cx = field.x + space::MD + 12;
    let cy = field.center_y() as i32;
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy;
    let d2 = dx * dx + dy * dy;
    let ring = (36..=81).contains(&d2);
    let handle = (5..=11).contains(&dx) && (dx - dy).abs() <= 2 && dy >= 5;
    if ring || handle {
        return theme.text_muted;
    }

    let (raw, raw_len) = search_placeholder(state);
    let text_x = field.x + space::XXL + space::MD;
    let room = field
        .w
        .saturating_sub((text_x - field.x) + space::MD);
    let (buf, n) = text::ellipsize(&raw[..raw_len], LABEL_SCALE, room);
    let y0 = field.center_y() - text::line_height(LABEL_SCALE) / 2;
    if let Some(c) = test_text(x, y, text_x, y0, LABEL_SCALE, &buf[..n], theme.text_muted) {
        return c;
    }
    theme.surface_raised
}

/// The filter chip row.
///
/// WHY one chip: the package registry carries no category metadata, so a row of
/// "All / Work / Personal / Tools" would name filters that do not exist and switch
/// between nothing. One chip, labelled with the live count, is the only category
/// this screen can honestly offer.
fn filter_chips(x: usize, y: usize, state: &LauncherState, row: &Rect, theme: &Theme) -> u32 {
    let (raw, raw_len) = chip_label(state);
    let w = (text::measure(&raw[..raw_len], LABEL_SCALE) + space::MD * 2).min(row.w);
    let chip = Rect::new(row.x, row.y, w, row.h);
    if !inside_rounded(chip, x, y, radius::CONTROL) {
        return theme.bg;
    }
    if !inside_rounded(chip.inset(1), x, y, radius::CONTROL - 1) {
        return theme.accent;
    }
    let ink = layout::ink_on(theme, theme.surface);
    let (buf, n) = text::ellipsize(
        &raw[..raw_len],
        LABEL_SCALE,
        chip.w.saturating_sub(space::MD * 2),
    );
    let tx = chip.x + text::center_x(&buf[..n], LABEL_SCALE, chip.w);
    let ty = chip.center_y() - text::line_height(LABEL_SCALE) / 2;
    if let Some(c) = test_text(x, y, tx, ty, LABEL_SCALE, &buf[..n], ink) {
        return c;
    }
    theme.surface
}

/// The dot row: one dot per page, filled for the one being shown.
///
/// WHY a computed row and not a fixed three: the old code clamped the count to
/// three, so a thirty-app catalogue silently lost its fourth page from the
/// indicator even though the grid above it paged correctly.
fn pagination_dots(
    x: usize,
    y: usize,
    state: &LauncherState,
    row: &Rect,
    total: usize,
    theme: &Theme,
) -> u32 {
    let pitch = DOT_D + DOT_GAP;
    let span = total * pitch - DOT_GAP;
    let x0 = row.x + (row.w - span) / 2;
    let cy = row.center_y();
    let current = app_drawer_page(state);
    for page in 0..total {
        let cx = x0 + page * pitch + DOT_D / 2;
        let dx = x as i64 - cx as i64;
        let dy = y as i64 - cy as i64;
        let d2 = dx * dx + dy * dy;
        let r = if page == current {
            DOT_D / 2 + 1
        } else {
            DOT_D / 2
        } as i64;
        if d2 <= r * r {
            return if page == current {
                theme.text
            } else {
                theme.text_muted
            };
        }
    }
    theme.bg
}

/// The catalogue with nothing in it.
///
/// WHY a message rather than an empty grid: `layout::grid` deliberately keeps a
/// zero-item grid addressable so callers cannot divide by zero, which means a drawer
/// with no apps would otherwise render four rows of empty cards and a single dot --
/// a layout that looks broken rather than empty.
fn empty_catalogue(x: usize, y: usize, area: &Rect, theme: &Theme) -> u32 {
    const HEAD: &[u8] = b"No apps yet";
    const BODY: &[u8] = b"No active packages are installed.";

    let head_h = text::line_height(type_scale::TITLE);
    let body_h = text::line_height(LABEL_SCALE);
    let block_h = head_h + space::MD + body_h;
    let top = area.y + area.h.saturating_sub(block_h) / 2;
    let room = area.w.saturating_sub(space::XXL * 2);

    let head_x = area.x + text::center_x(HEAD, type_scale::TITLE, area.w);
    if let Some(c) = test_text(x, y, head_x, top, type_scale::TITLE, HEAD, theme.text) {
        return c;
    }
    let body_y = top + head_h + space::MD;
    let (breaks, lines) = text::wrap(BODY, LABEL_SCALE, room, 2);
    let mut start = 0usize;
    for line in 0..lines {
        let len = breaks[line];
        let lx = area.x + text::center_x(&BODY[start..start + len], LABEL_SCALE, area.w);
        if let Some(c) = test_text(
            x,
            y,
            lx,
            body_y + line * (body_h + space::XS),
            LABEL_SCALE,
            &BODY[start..start + len],
            theme.text_muted,
        ) {
            return c;
        }
        start += len;
        if start < BODY.len() && BODY[start] == b' ' {
            start += 1;
        }
    }
    theme.bg
}

/// One app tile: a card, an icon disc with the app's badge, and its name.
fn app_tile(
    x: usize,
    y: usize,
    state: &LauncherState,
    g: &layout::Grid,
    slot: usize,
    start: usize,
    theme: &Theme,
) -> u32 {
    let Some(cell) = layout::grid_cell(g, slot) else {
        return theme.bg;
    };
    if !inside_rounded(cell, x, y, radius::CARD) {
        return theme.bg;
    }
    if !inside_rounded(cell.inset(1), x, y, radius::CARD - 1) {
        return theme.border;
    }
    let index = start + slot;

    // The icon and the name are one group, centred in the tile, so a tile that is
    // taller than its content stays balanced rather than hanging from the top.
    let icon_d = cell.w.saturating_sub(space::XXL * 2);
    let label_h = text::line_height(LABEL_SCALE);
    let group_h = icon_d + space::MD + label_h;
    let top = cell.y + cell.h.saturating_sub(group_h) / 2;
    let disc = Rect::new(cell.center_x() - icon_d / 2, top, icon_d, icon_d);

    let dx = x as i32 - disc.center_x() as i32;
    let dy = y as i32 - disc.center_y() as i32;
    let radius_px = (icon_d / 2) as i32;
    if dx * dx + dy * dy <= radius_px * radius_px {
        let hue = hue_for(theme, index);
        let badge = app_badge(state, index);
        let bx = disc.x + text::center_x(badge, BADGE_SCALE, icon_d);
        let by = disc.center_y() - text::line_height(BADGE_SCALE) / 2;
        if let Some(c) = test_text(x, y, bx, by, BADGE_SCALE, badge, layout::ink_on(theme, hue))
        {
            return c;
        }
        return hue;
    }

    // WHY ellipsize rather than draw the name whole: the label buffer is sixteen
    // bytes and the registry has no idea how wide the tile is, so a long name used
    // to be drawn at its full width straight through the neighbouring tile.
    let label = app_label(state, index);
    let (buf, n) = text::ellipsize(
        label,
        LABEL_SCALE,
        cell.w.saturating_sub(space::MD * 2),
    );
    let tx = cell.x + text::center_x(&buf[..n], LABEL_SCALE, cell.w);
    let ty = top + icon_d + space::MD;
    if let Some(c) = test_text(x, y, tx, ty, LABEL_SCALE, &buf[..n], theme.text) {
        return c;
    }
    theme.surface
}

/// Whether a pixel belongs to a rectangle with rounded corners.
///
/// WHY corners are rasterised here: `radius::CARD` and friends are design tokens,
/// and a card that ignores them reads as a table cell. With no 2D drawing API in the
/// shell, a distance test against the four corner circles is the only way to get a
/// round corner out of a per-pixel renderer.
fn inside_rounded(rect: Rect, x: usize, y: usize, r: usize) -> bool {
    if !rect.contains(x, y) {
        return false;
    }
    if r == 0 {
        return true;
    }
    let r = r.min(rect.w / 2).min(rect.h / 2);
    let left = rect.x + r;
    let right = rect.x + rect.w - 1 - r;
    let top = rect.y + r;
    let bottom = rect.y + rect.h - 1 - r;
    if !(x < left || x > right) || !(y < top || y > bottom) {
        return true;
    }
    let cx = if x < left { left } else { right };
    let cy = if y < top { top } else { bottom };
    let dx = x as i64 - cx as i64;
    let dy = y as i64 - cy as i64;
    dx * dx + dy * dy <= (r as i64) * (r as i64)
}

// -----------------------------------------------------------------------------
// Diagnostics
// -----------------------------------------------------------------------------

/// The render-complete log line, every count in it taken from the catalogue.
///
/// WHY it is built here and not written in the shell: the previous line was a
/// literal -- "16-app grid presented" -- that said the same thing with three apps
/// installed and with thirty. The lifecycle harness waits for the
/// "App drawer surface rendered." tail, which is kept byte for byte.
#[allow(dead_code)]
pub fn app_drawer_log(state: &LauncherState) -> ([u8; APP_DRAWER_LOG_CAP], usize) {
    let mut buf = [0u8; APP_DRAWER_LOG_CAP];
    let mut at = 0usize;
    put(
        &mut buf,
        &mut at,
        b"[LAUNCHER] render_app_drawer complete: ",
    );
    if state.app_count == 0 {
        put(&mut buf, &mut at, b"0 apps in the package registry; empty state shown. ");
    } else {
        push_usize(&mut buf, &mut at, state.app_count);
        put(&mut buf, &mut at, b" apps installed, page ");
        push_usize(&mut buf, &mut at, app_drawer_page(state) + 1);
        put(&mut buf, &mut at, b" of ");
        push_usize(&mut buf, &mut at, app_drawer_page_count(state));
        let capacity = app_drawer_per_page(state);
        let (start, end) = layout::page_slice(state.app_count, capacity, app_drawer_page(state));
        put(&mut buf, &mut at, b" showing ");
        push_usize(&mut buf, &mut at, end - start);
        put(&mut buf, &mut at, b" of ");
        push_usize(&mut buf, &mut at, state.app_count);
        put(&mut buf, &mut at, b" across ");
        push_usize(&mut buf, &mut at, drawer_columns(state));
        put(&mut buf, &mut at, b" columns. ");
    }
    put(&mut buf, &mut at, LOG_TAIL);
    (buf, at)
}

/// Emits [`app_drawer_log`] through the serial console.
///
/// WHY this exists: the shell's `render_view` logs one literal per view, so making
/// that line describe the catalogue means replacing the `ViewMode::AppDrawer` arm
/// with a call to this function. Doing it here keeps the message next to the layout
/// that produced it.
#[allow(dead_code)]
pub fn log_render(state: &LauncherState) {
    let (buf, len) = app_drawer_log(state);
    let msg = core::str::from_utf8(&buf[..len]).unwrap_or(
        "[LAUNCHER] render_app_drawer complete. App drawer surface rendered.",
    );
    let _ = interos_std::log(0, msg);
}