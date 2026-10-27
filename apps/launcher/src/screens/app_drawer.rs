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

use crate::{test_text_aa, LauncherState};
use launcher::layout::{self, Rect};
use launcher::text;
use launcher::theme::{
    space, type_scale, Theme, CONTENT_PADDING, NAV_BAR_HEIGHT,
    SCREEN_HEIGHT, SCREEN_WIDTH, STATUS_BAR_HEIGHT,
};

/// Rows the drawer shows at once.
///
/// WHY four: the band between the filter strip and the dot row is 1,072px tall, and
/// a tile holds an icon, a gap and a line of name. Four rows keep the tile close to
/// square (196x256 at three columns) instead of letting a two-row catalogue stretch
/// each tile to 544px. It is a viewport capacity, not a description of the
/// catalogue: what a page holds is `columns * DRAWER_ROWS`.
const DRAWER_ROWS: usize = 5;

/// Gap between tiles, off the theme's spacing scale.
const TILE_GUTTER: usize = 20;

/// Longest name the catalogue can carry: the element width of the label buffer in
/// `LauncherState::app_labels`. Read from the state layout rather than assumed.
const APP_LABEL_CAPACITY: usize = 16;

/// Type scale for an app's name, and for the badge glyph inside its icon.
const LABEL_SCALE: usize = type_scale::BODY;
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
    let search = Rect::new(content.x, 56, content.w, 48);
    let chips = Rect::new(content.x, 116, content.w, 44);
    let dots = Rect::new(
        content.x,
        1095,
        content.w,
        space::XL,
    );
    let grid = Rect::new(
        50,
        175,
        620,
        895,
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

/// Rows the category chips occupy.
///
/// WHY an accessor: the shell repaints these rows when the active tab changes,
/// and the rectangle is `frame().chips` -- the same one `app_drawer_chip_at`
/// hit-tests -- so the band and the pixels are one number. See [`pages_band`]
/// for the grid's.
pub fn chips_band() -> (usize, usize) {
    let f = frame();
    (f.chips.y, f.chips.y + f.chips.h)
}

/// Rows the page grid and the pagination dots occupy.
///
/// WHY one band and not one per control: a page change redraws every tile and
/// moves the current-page dot, and the gap between the grid's bottom and the
/// dots is background that is rewritten identically. Two separate writes would
/// save 25 rows of pure background at the cost of an extra surface write.
///
/// WHY the chips are NOT in this band: they are a separate row and a page change
/// cannot move them, so including them would repaint a row that is already
/// correct.
pub fn pages_band() -> (usize, usize) {
    let f = frame();
    (f.grid.y, f.dots.y + f.dots.h)
}

/// Columns the drawer lays out: strictly 3 columns (3x4 grid) matching Screen 3.
pub fn drawer_columns(_state: &LauncherState) -> usize {
    3
}

/// How many apps one page of the drawer holds for the current catalogue.
pub fn app_drawer_per_page(state: &LauncherState) -> usize {
    drawer_columns(state) * DRAWER_ROWS
}

/// Category tabs: All, Work, Personal, Tools.
pub const APP_DRAWER_TAB_NAMES: [&[u8]; 4] = [b"All", b"Work", b"Personal", b"Tools"];

fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| {
        w.iter().zip(needle.iter()).all(|(&h, &n)| {
            h.to_ascii_lowercase() == n.to_ascii_lowercase()
        })
    })
}

/// Filters installed application indices matching the specified category tab and search query.
pub fn filtered_indices(state: &LauncherState, tab: usize, out: &mut [usize; 32]) -> usize {
    let mut count = 0;
    for i in 0..state.app_count.min(32) {
        let id_len = (state.app_id_lens[i] as usize).min(state.app_ids[i].len());
        let id = &state.app_ids[i][..id_len];
        if id == b"org.interos.launcher" || id == b"launcher"
            || id == b"org.interos.compositor" || id == b"compositor"
            || id == b"org.interos.init" || id == b"init" || id == b"user_init"
            || id == b"org.interos.card_demo" || id == b"card_demo" || id == b"app_card_demo"
        {
            continue;
        }
        let matches_tab = if tab == 0 {
            true
        } else {
            let cat_len = state.app_category_lens[i] as usize;
            let cat = &state.app_categories[i][..cat_len];
            match tab {
                1 => cat == b"Work",
                2 => cat == b"Personal",
                3 => cat == b"Tools",
                _ => true,
            }
        };
        let matches_search = if state.search_query_len == 0 {
            true
        } else {
            let q = &state.search_query[..state.search_query_len];
            let label_len = (state.app_label_lens[i] as usize).min(state.app_labels[i].len());
            let label = &state.app_labels[i][..label_len];
            contains_ignore_case(label, q) || contains_ignore_case(id, q)
        };
        if matches_tab && matches_search {
            out[count] = i;
            count += 1;
        }
    }
    count
}

/// How many pages the current catalogue needs.
pub fn app_drawer_page_count(state: &LauncherState) -> usize {
    let mut filtered = [0usize; 32];
    let count = filtered_indices(state, state.app_drawer_tab, &mut filtered);
    layout::pages(count, app_drawer_per_page(state))
}

/// The page actually shown, clamped into range.
pub fn app_drawer_page(state: &LauncherState) -> usize {
    let pages = app_drawer_page_count(state);
    if pages == 0 {
        0
    } else {
        state.app_drawer_page.min(pages - 1)
    }
}

/// The cell rectangle for slot `slot` of the current page, at the drawer's stable
/// cell size.
fn current_grid(capacity: usize) -> layout::Grid {
    layout::grid_fixed_columns(capacity, frame().grid, 3, DRAWER_ROWS, TILE_GUTTER)
}

/// Which catalogue index a touch at (`x`, `y`) lands on, if any.
///
/// WHY this lives here and not in the shell: the shell's own hit test re-derived
/// the grid from hardcoded row and column bands, so any change to the layout would
/// have left taps pointing at the wrong app. This is the same geometry the screen
/// draws with, so the two cannot drift.
#[allow(dead_code)]
pub fn app_drawer_hit(state: &LauncherState, x: usize, y: usize) -> Option<usize> {
    let mut filtered = [0usize; 32];
    let count = filtered_indices(state, state.app_drawer_tab, &mut filtered);
    if count == 0 {
        return None;
    }
    let capacity = app_drawer_per_page(state);
    let g = current_grid(capacity);
    let slot = slot_at(&g, x, y)?;
    let (start, end) = layout::page_slice(count, capacity, app_drawer_page(state));
    if slot >= end - start {
        return None;
    }
    Some(filtered[start + slot])
}

/// Which category filter chip was tapped at (`x`, `y`), if any.
#[allow(dead_code)]
pub fn app_drawer_chip_at(x: usize, y: usize) -> Option<usize> {
    let f = frame();
    if !f.chips.contains(x, y) {
        return None;
    }
    let gap = 10usize;
    let chip_w = (f.chips.w.saturating_sub(gap * 3)) / 4;
    for i in 0..4 {
        let cx = f.chips.x + i * (chip_w + gap);
        if (cx..cx + chip_w).contains(&x) {
            return Some(i);
        }
    }
    None
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
    let x0 = f.dots.x + f.dots.w.saturating_sub(span) / 2;
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

/// Grid slot under a point, restricted to the drawn cells.
///
/// WHY the gutters are not rounded to a neighbour: the old hit test shifted
/// the origin by the touch-slop tokens and clamped every point to a cell, so a
/// tap in the gap between two tiles launched an app the user did not touch.
/// A gutter is empty space and now answers with nothing.
fn slot_at(g: &layout::Grid, x: usize, y: usize) -> Option<usize> {
    let pitch_x = g.cell.w + g.gutter;
    let pitch_y = g.cell.h + g.gutter;
    if pitch_x == 0 || pitch_y == 0 {
        return None;
    }
    if x < g.origin_x || y < g.origin_y {
        return None;
    }
    let rel_x = x - g.origin_x;
    let rel_y = y - g.origin_y;
    let col = rel_x / pitch_x;
    let row = rel_y / pitch_y;
    if col >= g.columns || row >= g.rows {
        return None;
    }
    if rel_x % pitch_x >= g.cell.w || rel_y % pitch_y >= g.cell.h {
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

fn app_id(state: &LauncherState, index: usize) -> &[u8] {
    if index >= state.app_count {
        return &[];
    }
    let len = (state.app_id_lens[index] as usize).min(state.app_ids[index].len());
    &state.app_ids[index][..len]
}

/// The rotation an app's icon is drawn in.
///
/// WHY four and not six: the rotation reads from theme fields only, and the theme
/// has exactly four chromatic tokens. With three columns no two adjacent tiles can
/// share a rotation, so the grid still reads as distinct apps.
fn hue_for(theme: &Theme, index: usize) -> u32 {
    const ROTATION_KEY: usize = 4;
    match index % ROTATION_KEY {
        0 => theme.primary,
        1 => theme.success,
        2 => theme.warning,
        _ => theme.error,
    }
}

/// Rich, authentic background color for each app's squircle icon matching Screen 3.
fn icon_color(kind: Option<launcher::icons::IconKind>, index: usize, theme: &Theme) -> u32 {
    use launcher::icons::IconKind;
    match kind {
        Some(IconKind::Phone) => 0xFF34C759,
        Some(IconKind::Messages) => 0xFF30D158,
        Some(IconKind::Camera) => 0xFF2C2C2E,
        Some(IconKind::Browser) => 0xFF007AFF,
        Some(IconKind::Files) => 0xFF0A84FF,
        Some(IconKind::Gallery) => 0xFF32ADE6,
        Some(IconKind::Settings) => 0xFF8E8E93,
        Some(IconKind::Music) => 0xFFFF2D55,
        Some(IconKind::Calculator) => 0xFFFF9500,
        Some(IconKind::Health) => 0xFFFF375F,
        Some(IconKind::Vault) => 0xFF5856D6,
        Some(IconKind::MeshChat) => 0xFF007AFF,
        Some(IconKind::Calendar) => 0xFFFF3B30,
        Some(IconKind::Notes) => 0xFFFFCC00,
        Some(IconKind::Contacts) => 0xFF007AFF,
        Some(IconKind::Maps) => 0xFF34C759,
        Some(IconKind::Apps) => 0xFF5856D6,
        Some(IconKind::Briefcase) => 0xFFD97706,
        Some(IconKind::CardDemo) => 0xFF5856D6,
        Some(IconKind::Sun) => 0xFFFF9500,
        Some(IconKind::CheckCircle) => 0xFF34C759,
        Some(IconKind::AgentManager) => 0xFF7C3AED,
        Some(IconKind::VideoPlayer) => 0xFFFF453A,
        None => hue_for(theme, index),
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
#[allow(dead_code)]
fn search_placeholder(state: &LauncherState) -> ([u8; 32], usize) {
    let mut buf = [0u8; 32];
    let n = join_count(b"Search ", state.app_count, b" apps", &mut buf);
    (buf, n)
}

/// The filter chip's label: the whole catalogue, counted.
#[allow(dead_code)]
fn chip_label(state: &LauncherState) -> ([u8; 32], usize) {
    let mut buf = [0u8; 32];
    let n = join_count(b"All ", state.app_count, b" apps", &mut buf);
    (buf, n)
}

// -----------------------------------------------------------------------------
// Runs and the per-frame frame
// -----------------------------------------------------------------------------

/// Capacity of one run's byte buffer. WHY it matches `text::ellipsize`, so a run
/// built once per frame is byte-for-byte the run the per-pixel painter built.
const RUN_CAP: usize = 96;

/// One label, placed and measured once per frame.
///
/// WHY this type: `filter_chips` measured and ellipsized its own label for every
/// pixel of the chip row, `search_field` did the same for every pixel of the search
/// field, and `app_tile` did it twice more for every pixel of a tile -- with an
/// `ellipsize` returning a 96-byte buffer by value each time. Worse, deciding the
/// chip's own width needed `text::measure`, and deciding which page was showing
/// needed `drawer_columns`, which walks the catalogue's label lengths to find the
/// median. All of that is a function of the state and of nothing per pixel.
///
/// WHY the box is carried as well as the origin: `test_text` walks the pen of every
/// glyph before it can answer "no", so a pixel nowhere near a run still costs a pen
/// walk per label. The stored box is exactly
/// `y .. y + line_height(scale)` by `x .. x + measure(run)`, which is the region
/// `test_text` can return `Some` for.
#[allow(dead_code)]
struct Run {
    x: usize,
    y: usize,
    w: usize,
    scale: usize,
    len: usize,
    ink: u32,
    /// Pixels per em for this run's outline rendering.
    ///
    /// WHY a separate number from `scale`: the bitmap face is a 16x24 cell
    /// multiplied by `scale`, so its cap height is 17*`scale` pixels. The
    /// outline face is rasterised at an em, and its cap height is 1456 of 2048
    /// em, so the em that matches the same cap height is
    /// `17 * scale * 2048 / 1456`. `outline_text::em_for_scale` is that
    /// derivation, kept in one place so every screen that migrates a run
    /// derives it the same way.
    #[allow(dead_code)]
    em: u32,
    /// Top row of the box that covers BOTH renderers.
    ///
    /// WHY the box is a union: the outline run's line box is not the bitmap
    /// cell. At the em matching a scale-2 cap the outline's ascent reaches 50
    /// rows above the run's baseline while the bitmap's own ascender reaches 4
    /// rows into its cell, so a box measured for the cell alone rejects rows
    /// the outline inks and the text is silently clipped. `home.rs` hit exactly
    /// this and the lesson is carried here.
    top: usize,
    /// Height of that union box.
    box_h: usize,
    bytes: [u8; RUN_CAP],
    outline: Option<launcher::outline_text::OutlineRun>,
}

impl Run {
    /// A run that never inks anything.
    const EMPTY: Run = Run {
        x: 0,
        y: 0,
        w: 0,
        scale: 1,
        len: 0,
        ink: 0,
        em: 0,
        top: 0,
        box_h: 0,
        bytes: [0u8; RUN_CAP],
        outline: None,
    };

    /// `bytes` at (`x`, `y`), truncated with an ellipsis if wider than `fit` --
    /// what the old painters did per pixel.
    fn fitted(x: usize, y: usize, scale: usize, fit: usize, bytes: &[u8], ink: u32) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// As [`Run::fitted`], centred across `centre` from `x`.
    ///
    /// WHY two widths: a tile fits its name to the cell less its padding and then
    /// centres the result across the whole cell, so the two are not the same number.
    fn centred(
        x: usize,
        y: usize,
        scale: usize,
        fit: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        let (buf, len) = text::ellipsize(bytes, scale, fit);
        let inset = text::center_x(&buf[..len], scale, centre);
        Run::placed(x + inset, y, scale, buf, len, ink)
    }

    /// A run drawn whole, with no truncation step, centred across `centre` from `x`.
    ///
    /// WHY the empty catalogue's headline uses this and not [`Run::centred`]: the old
    /// painter measured and centred it without ever offering it a width to fit.
    #[allow(dead_code)]
    fn centred_whole(
        x: usize,
        y: usize,
        scale: usize,
        centre: usize,
        bytes: &[u8],
        ink: u32,
    ) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        let inset = text::center_x(&buf[..len], scale, centre);
        Run::placed(x + inset, y, scale, buf, len, ink)
    }

    /// A run drawn whole, with no truncation step -- for the badge word, which the
    /// old painter sized the type scale to fit and then handed straight to
    /// `test_text`.
    fn whole(x: usize, y: usize, scale: usize, bytes: &[u8], ink: u32) -> Run {
        let mut buf = [0u8; RUN_CAP];
        let len = bytes.len().min(RUN_CAP);
        buf[..len].copy_from_slice(&bytes[..len]);
        Run::placed(x, y, scale, buf, len, ink)
    }

    /// A run from an already-resolved buffer.
    ///
    /// WHY the width and the box are wider than the bitmap's own: the run now
    /// has two renderers, and the box has to admit both. The bitmap's measure
    /// and line height bound one; the outline's advance and its ascent-plus-
    /// descent bound the other. Taking the union of each costs nothing at this
    /// point -- it runs once per frame per run -- and is the difference between
    /// text that renders and text that is silently clipped.
    ///
    /// WHY the union width is measured rather than assumed: the outline face is
    /// condensed and is not always the narrower of the two. "System" measures
    /// 67 bitmap advances at scale 3 (a 201px measure) and a 206px outline
    /// advance, so the outline is the wider face on this very surface's
    /// vocabulary and a bitmap-only box would clip it.
    fn placed(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32) -> Run {
        Self::placed_styled(x, y, scale, buf, len, ink, true)
    }

    fn placed_styled(x: usize, y: usize, scale: usize, buf: [u8; RUN_CAP], len: usize, ink: u32, bold: bool) -> Run {
        let text = &buf[..len];
        let em = if scale >= 3 {
            34
        } else if scale >= 2 {
            26
        } else {
            launcher::outline_text::em_for_scale(scale)
        };
        let (asc, desc) = interos_std::design::typeface::line_metrics(em);
        let baseline = (y + launcher::font::ascent_for_scale(scale)).max(asc as usize);
        let outline_advance = interos_std::design::typeface::text_advance_px(text, em) as usize + if bold { 1 } else { 0 };
        let top = baseline.saturating_sub(asc as usize);
        let outline = if scale >= 2 && len <= launcher::outline_text::KEY_MAX {
            launcher::outline_text::OutlineRun::place_styled(x, baseline, em, text, bold)
        } else {
            None
        };
        Run {
            x,
            y,
            w: text::measure(text, scale).max(outline_advance),
            scale,
            len,
            ink,
            em,
            top,
            box_h: (baseline + desc as usize).saturating_sub(top),
            bytes: buf,
            outline,
        }
    }

    /// The colour of (`px`, `py`) if this run inks it, anti-aliased against `bg`.
    ///
    /// WHY this exists beside the 1-bit path: the font rasteriser carries 4-bit
    /// coverage (`font::coverage`), and a hard-edged glyph over a flat card shows
    /// a stair-stepped outline that the reference's typography does not have.
    /// The blend is per covered pixel, so the cost is confined to the glyph.
    fn pixel_on(&self, px: usize, py: usize, bg: u32) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        if let Some(ref outline) = self.outline {
            return outline.pixel(px, py, self.ink, bg);
        }
        test_text_aa(px, py, self.x, self.y, self.scale, &self.bytes[..self.len], self.ink, bg)
    }
}

/// One app tile, with its cell, its icon disc, its rotation and both words resolved.
struct TilePlan {
    #[allow(dead_code)]
    cell: Rect,
    disc: Rect,
    disc_r: i32,
    /// The chromatic rotation for this slot's catalogue index. WHY it is here: it is
    /// derived from the catalogue index modulo the theme's four chromatic tokens,
    /// which meant reading a theme field per pixel.
    #[allow(dead_code)]
    hue: u32,
    icon: Option<launcher::icons::IconKind>,
    badge: Run,
    label: Run,
}

impl TilePlan {
    const EMPTY: TilePlan = TilePlan {
        cell: Rect::new(0, 0, 0, 0),
        disc: Rect::new(0, 0, 0, 0),
        disc_r: 0,
        hue: 0,
        icon: None,
        badge: Run::EMPTY,
        label: Run::EMPTY,
    };
}

/// The empty-catalogue message, resolved: a headline and up to two wrapped lines.
struct EmptyPlan {
    head: Run,
    body: [Run; 2],
    body_count: usize,
}

/// The status bar's chrome plan.
struct StatusPlan {
    clock: Option<Run>,
}

/// Tiles this drawer can hold on one page.
///
/// WHY it is `DRAWER_ROWS` times the grid's ceiling and not the current page size:
/// `layout::grid_columns` caps a surface at six columns, so a page can never hold
/// more than 24, and sizing the array by the catalogue would resize the plan under a
/// swipe.
const MAX_TILES: usize = DRAWER_ROWS * 6;

struct ChipPlan {
    rect: Rect,
    label: Run,
    active: bool,
}

/// Everything the painters need that does not depend on the pixel.
struct Paint {
    status: StatusPlan,
    search: Run,
    chips: [ChipPlan; 4],
    /// One dot per page: the first dot's centre and the current page.
    dots_x0: usize,
    dots_cy: usize,
    dots_total: usize,
    dots_current: usize,
    grid: layout::Grid,
    tiles: [TilePlan; MAX_TILES],
    /// How many of `tiles` this page fills; slots at or past it are background.
    shown: usize,
    empty: Option<EmptyPlan>,
    cached_tab: usize,
    cached_page: usize,
    cached_app_count: usize,
    cached_query_len: usize,
    /// The query bytes the plan was built from. Length alone is not enough:
    /// replacing "browel" with "browser" keeps the length and would otherwise
    /// reuse a stale result page.
    cached_query: [u8; 32],
}

/// Builds this frame's plan.
fn build(state: &LauncherState) -> Paint {
    let theme = launcher::theme::SHELL;
    let f = frame();

    let status = StatusPlan {
        clock: if state.clock_source_ok {
            Some(Run::whole(
                40,
                8,
                type_scale::BODY,
                &state.clock_time,
                theme.on_surface,
            ))
        } else {
            None
        },
    };

    // Search field: shows active query or catalogue placeholder
    let (search_text, search_text_len, search_color) = if state.search_query_len > 0 {
        let mut q_buf = [0u8; 36];
        let q_len = state.search_query_len.min(32);
        q_buf[..q_len].copy_from_slice(&state.search_query[..q_len]);
        q_buf[q_len] = b'|';
        (q_buf, q_len + 1, theme.on_surface)
    } else {
        let placeholder = b"Search apps...";
        let mut p_buf = [0u8; 36];
        p_buf[..placeholder.len()].copy_from_slice(placeholder);
        (p_buf, placeholder.len(), theme.on_surface_variant)
    };
    let search_x = f.search.x + space::XXL + space::MD;
    let search_room = f
        .search
        .w
        .saturating_sub((search_x - f.search.x) + space::MD);
    // Optical vertical centering: glyph center aligned exactly to search.center_y()
    let search_y = f.search.center_y().saturating_sub(9);
    let search = Run::fitted(
        search_x,
        search_y,
        type_scale::BODY,
        search_room,
        &search_text[..search_text_len],
        search_color,
    );

    // 4 Category Filter Chips: All, Work, Personal, Tools
    let gap = 10usize;
    let chip_w = (f.chips.w.saturating_sub(gap * 3)) / 4;
    let chip_h = 40usize;
    let chip_y = f.chips.y + (f.chips.h.saturating_sub(chip_h)) / 2;
    let mut chips = [
        ChipPlan { rect: Rect::new(0, 0, 0, 0), label: Run::EMPTY, active: false },
        ChipPlan { rect: Rect::new(0, 0, 0, 0), label: Run::EMPTY, active: false },
        ChipPlan { rect: Rect::new(0, 0, 0, 0), label: Run::EMPTY, active: false },
        ChipPlan { rect: Rect::new(0, 0, 0, 0), label: Run::EMPTY, active: false },
    ];
    for (i, label_bytes) in APP_DRAWER_TAB_NAMES.iter().enumerate() {
        let chip_x = f.chips.x + i * (chip_w + gap);
        let chip_rect = Rect::new(chip_x, chip_y, chip_w, chip_h);
        let active = state.app_drawer_tab == i;
        let ink = if active {
            0xFFFFFFFF
        } else {
            theme.on_surface_variant
        };
        // Optical vertical centering: glyph center aligned exactly to chip_rect.center_y()
        let chip_label_y = chip_rect.center_y().saturating_sub(9);
        let label = Run::centred(
            chip_rect.x,
            chip_label_y,
            type_scale::BODY,
            chip_rect.w.saturating_sub(space::SM * 2),
            chip_rect.w,
            label_bytes,
            ink,
        );
        chips[i] = ChipPlan {
            rect: chip_rect,
            label,
            active,
        };
    }

    // Grid and this page's filtered tiles.
    let mut filtered = [0usize; 32];
    let filtered_count = filtered_indices(state, state.app_drawer_tab, &mut filtered);
    let capacity = app_drawer_per_page(state);
    let page_total = layout::pages(filtered_count, capacity);
    let page = app_drawer_page(state);
    let (start, end) = layout::page_slice(filtered_count, capacity, page);
    let grid = current_grid(capacity);
    let mut tiles = [TilePlan::EMPTY; MAX_TILES];
    let shown = (end - start).min(MAX_TILES);
    for slot in 0..shown {
        let index = filtered[start + slot];
        let Some(cell) = layout::grid_cell(&grid, slot) else {
            continue;
        };
        let icon_d = 80usize;
        let group_h = icon_d + 10 + text::line_height(LABEL_SCALE);
        let top = cell.y + cell.h.saturating_sub(group_h) / 2;
        let disc = Rect::new(cell.center_x() - icon_d / 2, top, icon_d, icon_d);
        let badge = app_badge(state, index);
        let id = app_id(state, index);
        let label = app_label(state, index);
        let icon_kind = launcher::icons::match_icon_kind(id)
            .or_else(|| launcher::icons::match_icon_kind(label))
            .or_else(|| launcher::icons::match_icon_kind(badge));
        let hue = icon_color(icon_kind, index, &theme);

        let badge_run = if icon_kind.is_some() || badge.is_empty() {
            Run::EMPTY
        } else {
            Run::whole(
                disc.x + text::center_x(badge, BADGE_SCALE, icon_d),
                disc.center_y() - text::line_height(BADGE_SCALE) / 2,
                BADGE_SCALE,
                badge,
                theme.on_surface,
            )
        };

        tiles[slot] = TilePlan {
            cell,
            disc,
            disc_r: 18,
            hue,
            icon: icon_kind,
            badge: badge_run,
            label: Run::centred(
                cell.x,
                top + icon_d + 10,
                LABEL_SCALE,
                cell.w + 16,
                cell.w,
                label,
                theme.on_surface,
            ),
        };
    }

    let empty = if filtered_count == 0 {
        let searching = state.search_query_len > 0;
        let (head_bytes, body_bytes) = if searching {
            (
                &b"No matching apps"[..],
                &b"Try another name or clear the tab filter"[..],
            )
        } else {
            match state.app_drawer_tab {
                1 => (&b"No Work Apps"[..], &b"No work applications in this category"[..]),
                2 => (&b"No Personal Apps"[..], &b"No personal applications in this category"[..]),
                3 => (&b"No Tool Apps"[..], &b"No tool applications in this category"[..]),
                _ => (&b"No apps installed"[..], &b"The app catalogue is empty"[..]),
            }
        };
        let head_y = f.grid.y + 32;
        let body_y = head_y + text::line_height(type_scale::TITLE) + 20;
        Some(EmptyPlan {
            head: Run::whole(f.grid.x, head_y, type_scale::TITLE, head_bytes, theme.on_surface),
            body: [
                Run::whole(f.grid.x, body_y, type_scale::BODY, body_bytes, theme.on_surface_variant),
                Run::EMPTY,
            ],
            body_count: 1,
        })
    } else {
        None
    };

    let pitch = DOT_D + DOT_GAP;
    let span = page_total * pitch - DOT_GAP;
    Paint {
        status,
        search,
        chips,
        dots_x0: f.dots.x + f.dots.w.saturating_sub(span) / 2,
        dots_cy: f.dots.center_y(),
        dots_total: page_total,
        dots_current: page,
        grid,
        tiles,
        shown,
        empty,
        cached_tab: state.app_drawer_tab,
        cached_page: page,
        cached_app_count: state.app_count,
        cached_query_len: state.search_query_len,
        cached_query: state.search_query,
    }
}

/// This frame's plan.
static mut PAINT: Option<Paint> = None;

/// Invalidates the App Drawer paint cache so dynamic app additions, tab changes,
/// and pagination updates immediately re-plan and redraw.
pub fn invalidate_app_drawer_frame() {
    unsafe {
        let slot: *mut Option<Paint> = core::ptr::addr_of_mut!(PAINT);
        *slot = None;
    }
}

fn paint(x: usize, y: usize, state: &LauncherState) -> &'static Paint {
    let slot: *mut Option<Paint> = core::ptr::addr_of_mut!(PAINT);
    let page = app_drawer_page(state);
    let should_rebuild = unsafe {
        match (*slot).as_ref() {
            None => true,
            Some(p) => {
                (x == 0 && y == 0)
                    || p.cached_page != page
                    || p.cached_tab != state.app_drawer_tab
                    || p.cached_app_count != state.app_count
                    || p.cached_query_len != state.search_query_len
                    || p.cached_query != state.search_query
            }
        }
    };
    if should_rebuild {
        unsafe { *slot = Some(build(state)) };
    }
    unsafe { (*slot).as_ref() }.expect("WHY: the slot is filled when it is empty")
}

// -----------------------------------------------------------------------------
// Render
// -----------------------------------------------------------------------------

#[inline(never)]
pub fn get_app_drawer_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    // WHY bound here rather than in a static: the shell has exactly one active
    // theme and every pixel on this screen resolves against it. A static would be a
    // second source of truth that no runtime setting could move. The shell should
    // hand the active theme in -- `LauncherState` grows a `theme: Theme` field fed
    // from the same setting that picks the wallpaper, and this line becomes
    // `state.theme`. Until then the binding is a value, not a constant, so the swap
    // is one edit in one place.
    let theme = launcher::theme::SHELL;

    // WHY the bands are recomputed rather than read from the plan: the shell asks
    // `drawer_hit` -- which rebuilds `frame()` -- whether a tap landed on the search
    // field, the chip, a page dot or a tile, and the two answers have to come from
    // the same geometry. It is a handful of rectangles, not a text metric.
    let f = frame();

    if y < STATUS_BAR_HEIGHT {
        return status_bar(x, y, &theme, &paint(x, y, state).status);
    }
    if f.pill.contains(x, y) {
        return theme.on_surface_variant;
    }
    if f.search.contains(x, y) {
        return search_field(x, y, &f.search, &theme, paint(x, y, state));
    }
    if f.chips.contains(x, y) {
        return filter_chips(x, y, &theme, paint(x, y, state));
    }

    // WHY the plan is taken before the empty-catalogue test: which case this pixel is
    // in is decided by `state.app_count`, and that answer is already resolved.
    let p = paint(x, y, state);
    if let Some(empty) = p.empty.as_ref() {
        return empty_catalogue(x, y, &theme, empty);
    }

    // Pager dots, and only when there is more than one page: a single dot says "you
    // are on page one of one", which is a fact nobody needs.
    if p.dots_total > 1 && f.dots.contains(x, y) {
        return pagination_dots(x, y, &theme, p);
    }

    let slot = slot_at(&p.grid, x, y).unwrap_or(usize::MAX);
    if slot >= p.shown {
        // A short final page leaves real slots empty; they show the page, not a blank
        // card that pretends to be an app.
        return theme.surface;
    }
    app_tile(x, y, &theme, &p.tiles[slot])
}


/// Status bar: the shell's one canonical bar.
fn status_bar(x: usize, y: usize, theme: &Theme, s: &StatusPlan) -> u32 {
    crate::screens::status::pixel_with(
        x,
        y,
        |x, y| s.clock.as_ref().and_then(|r| r.pixel_on(x, y, theme.surface)),
        theme,
    )
}

fn search_field(x: usize, y: usize, field: &Rect, theme: &Theme, p: &Paint) -> u32 {
    let rad = field.h / 2;
    if !inside_rounded(*field, x, y, rad) {
        return theme.surface;
    }
    if !inside_rounded(field.inset(1), x, y, rad - 1) {
        return theme.border;
    }
    // Magnifier. Ring plus handle
    let cx = field.x + space::MD + 12;
    let cy = field.center_y() as i32;
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy;
    let d2 = dx * dx + dy * dy;
    let ring = (36..=81).contains(&d2);
    let handle = (5..=11).contains(&dx) && (dx - dy).abs() <= 2 && dy >= 5;
    if ring || handle {
        return theme.on_surface_variant;
    }

    if let Some(c) = p.search.pixel_on(x, y, theme.surface_container_high) {
        return c;
    }
    theme.surface_container_high
}

/// The filter row: 4 category chips (All, Work, Personal, Tools) with crystal clear contrast.
fn filter_chips(x: usize, y: usize, theme: &Theme, p: &Paint) -> u32 {
    for chip in p.chips.iter() {
        if chip.rect.contains(x, y) {
            let rad = chip.rect.h / 2;
            if inside_rounded(chip.rect, x, y, rad) {
                let pill_fill = if chip.active {
                    0xFF2563EB
                } else {
                    theme.surface_container_high
                };
                if !chip.active && !inside_rounded(chip.rect.inset(1), x, y, rad.saturating_sub(1)) {
                    return theme.border;
                }
                if let Some(c) = chip.label.pixel_on(x, y, pill_fill) {
                    return c;
                }
                return pill_fill;
            }
        }
    }
    theme.surface
}

/// Page dots.
///
/// WHY drawn at all: a drawer that pages silently looks like a drawer that is full.
/// One dot per page, the current one filled, is the cheapest way to say "there are
/// more, and this is where you are".
///
/// WHY the count and the current page are already known: both come from the
/// catalogue through `drawer_columns`, which walks every installed label to find the
/// median width. That walk ran once per pixel of the dot row.
fn pagination_dots(x: usize, y: usize, theme: &Theme, p: &Paint) -> u32 {
    let pitch = (DOT_D + DOT_GAP) as i32;
    let mut cx = p.dots_x0 as i32;
    let cy = p.dots_cy as i32;
    for page in 0..p.dots_total {
        let dx = x as i32 - (cx + (DOT_D / 2) as i32);
        let dy = y as i32 - cy;
        let d2 = dx * dx + dy * dy;
        let r = if page == p.dots_current {
            (DOT_D / 2 + 1) as i32
        } else {
            (DOT_D / 2) as i32
        };
        if d2 <= r * r {
            return if page == p.dots_current {
                theme.on_surface
            } else {
                theme.on_surface_variant
            };
        }
        cx += pitch;
    }
    theme.surface
}

/// No packages are catalogued.
///
/// WHY this says so rather than showing the previous mock-up's category cards: every
/// card in it linked to a catalogue screen that does not exist, so all five were dead
/// controls, and a dead control is worse than an honest absence.
///
/// WHY the body is resolved here: it is a constant, but the old painter re-wrapped it
/// and re-centred each of its lines for every pixel of the grid area.
fn empty_catalogue(x: usize, y: usize, theme: &Theme, p: &EmptyPlan) -> u32 {
    if let Some(c) = p.head.pixel_on(x, y, theme.surface) {
        return c;
    }
    for line in 0..p.body_count {
        if let Some(c) = p.body[line].pixel_on(x, y, theme.surface) {
            return c;
        }
    }
    theme.surface
}

/// One app tile. Returns the page background for a pixel that is not on a tile.
///
/// WHY the label is ellipsized rather than drawn whole: `app_label` hands back the
/// registry's 16-byte label buffer, and the registry has no idea how wide the tile
/// is. A long name used to be drawn at its full width straight through the tile
/// beside it.
fn app_tile(x: usize, y: usize, theme: &Theme, t: &TilePlan) -> u32 {
    // 1. Icon squircle: vibrant authentic colored squircle with 18px corner radius
    if inside_rounded(t.disc, x, y, t.disc_r as usize) {
        if !inside_rounded(t.disc.inset(1), x, y, (t.disc_r as usize).saturating_sub(1)) {
            return theme.border;
        }
        if let Some(kind) = t.icon {
            let ink = 0xFFFFFFFF; // Pure crisp white icon on vibrant colored squircle
            if let Some(c) = launcher::icons::rasterize_icon_pixel_scaled_aa(kind, x, y, t.disc.center_x(), t.disc.center_y(), ink, t.hue, 22) {
                return c;
            }
        } else if let Some(c) = t.badge.pixel_on(x, y, t.hue) {
            return c;
        }
        return t.hue;
    }

    // 2. App name label
    if let Some(c) = t.label.pixel_on(x, y, theme.surface) {
        return c;
    }

    // 3. Clean background canvas
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
    let _ = interos_std::log(msg);
}
